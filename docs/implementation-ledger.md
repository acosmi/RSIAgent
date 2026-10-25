# RSIAgent 实施台账

唯一规范入口（方案正文仅本地）：`RSIAgent-v4.3定稿-工程实施方案-2026-10-12.md`。
plan_version：`v4.3`；plan_sha256：`73dd00bf5ef6e9757d0c391c9a8be4ca92b6e06a750aacb542817f82cbd208a7`。谱系：v4.1 `RSIAgent-v4.1定稿-工程实施方案-2026-09-19.md`（SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`，2026-09-30 归档至本地 `archive/2026-09-30-pre-v4.2/`）→ v4.2 `RSIAgent-v4.2定稿-工程实施方案-2026-09-30.md`（SHA-256 `70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455`，2026-10-12 归档至本地 `archive/2026-10-12-pre-v4.3/`）→ v4.3。历史验证记录保留其验证时的 plan_version/plan_sha256（真源 §18.9）。
本台账是实施与证据索引，不另立规范；历史 PR、测试和旧台账不决定当前规则。
用户于 2026-09-19 明确确认 v4.1 替代 v4，并授权将本台账上传远程；方案正文、内部合同、归档及 QA 原始材料只保留本地。2026-10-01 第四轮由 Codex 主控持续推进；实施子代理统一使用 gpt-6.1-sol，逐卡记录思考档位。每个任务独立 PR；主控全量复跑及至少两个反例通过后，以固定 head 按依赖顺序合并并核对合并树。历史轮次的合并操作者按各节原始记录保留。2026-10-11 起第五轮由 Claude Fable 5.1 主控，子代理按任务规模使用 Claude Sonnet 5.5 / Opus 5.5；2026-10-12 起第六阶段由 Claude Opus 5.5 主控，实施与侦察子代理一律 Claude Opus 5.5（effort high），验收通过的 PR 由主控按用户授权以固定 head admin 合并；第四轮二次验收结论见下一节。Actions派发、付费运行和部署/发布未授权。


## 2026-10-11 第四轮二次验收结论、合并协议偏离登记与第五轮首批返修队列（AG-094）

第五轮自 2026-10-11 起由 Claude Fable 5.1 主控（Claude Code），子代理按任务规模使用 Claude Sonnet 5.5 / Claude Opus 5.5；验收一律在文档化环境执行（默认 profile、默认测试线程栈），合并只用 `--merge` 并以固定 head 线性叠放。本节只折入已有结论，不改产品代码、索引或检查器。逐项记录保留在本地 out/audit-20260930/claude-r4-second-acceptance-20261011.md 与 ledger-draft-r5.md。

### 范围与方法
对第四轮（Codex 主控）合并到 main 的 PR #98–#141 共 44 项做独立二次验收：用 git/gh 逐项核对合并提交、验收 head 与树；核对 reports/support-scope.json 登记与 GitHub；解析第四轮主控全量日志、探针日志与 Task 回执；在全新 worktree 与全新 target 上按文档化环境（cards/_common.md 与 ctrl_verify.sh：CARGO_INCREMENTAL=0、DEV/TEST_DEBUG=0、opt-level 默认 0、不设 RUST_MIN_STACK）复跑 main fa570e0 的八项闸门；四个只读审查子代理分区审阅新增代码，High 级发现由主控静态核实。

### 结构性核验：通过
- 44 个合并提交的树均等于其验收 head 的树；除 #122 外，每个验收 head 的父提交等于合并时的 main。#122 以旧公开 head 0e8ff93b 三方合并，合并树等于第四轮本地叠放后验收的 head c2f89f42 的树（索引登记的 source_sha 也是 c2f89f42）。
- 索引 92 条记录（72 旧 + 20 新）；第四轮 35 个产品 PR 的 source_sha/merged_sha 与 GitHub 一致；未登记项为台账 PR（#98/#102/#107/#112/#115/#132/#141）、CI（#129）、前端（#139）与 Python 预算报告（#120）。检查器在 main：exit 0、structure_valid、errors 0、needs_verification 134（均为"manifest 断言未自动核 Git 祖先"或"旧记录缺本地输入清单"的边界说明，不是缺陷清单）。
- 第四轮清单提及的 88 个主控日志全部存在，26 个全量日志 failed=0；AG052–AG079 的 Rust/Python 探针日志在 head 通过、修缺陷类在父树失败；#120–#141 的 Task 回执 parent/tree 与 git 一致，Rust 通过数链自 1363（#120）升至 1729（#140）。
- 真源条款引用抽查无错引；fa570e0 树中无 out/、archive/ 与方案正文；Cargo.lock 与各 Cargo.toml 在 8043026..fa570e0 无改动；产品 PR 与台账 PR 互不越界。

### 文档化环境全量复跑：测试闸门未通过
- fmt、clippy -D warnings、build rsia、smoke_workspace、smoke_cli、support-scope 检查器、检查器单测 7 项退出码均为 0。
- `cargo test --workspace --locked` 退出 101：135 个测试组中 128 个正常结束（1627 通过 / 0 失败），7 个 evo-engine 测试二进制（exploration_recovery_counting_v42、exploration_trust_v42、exploration_v41、meta_trial_v42、monitoring、monitoring_consolidation_v42、step_terminal_classes_v42）在 optimization/exploration step 路径的测试处线程栈溢出中止，102 个测试未执行。仅加 RUST_MIN_STACK=16777216 复跑这 7 组，102 个全部通过，合计 1729，与第四轮计数一致，说明是 async 状态机栈帧过大而非无限递归。
- 原因：第四轮自 #130 起的全部验收回执都带未公开的 CARGO_PROFILE_DEV_OPT_LEVEL=1 与 CARGO_PROFILE_TEST_OPT_LEVEL=1，opt-level 1 缩小了栈帧、掩盖了溢出；第三轮末 8043026 在 opt-level 0 下通过。第五轮主控二分：#140 合并的父提交 c04850d7 上同 7 组在默认栈全部通过（99/0），fa570e0 上溢出，回归由 #140 引入。GitHub CI（ubuntu、默认 profile、2 MiB 测试线程栈）同样会失败。
- 判定：main fa570e0 在文档化环境下不满足测试闸门；第四轮"1729 全通过"只在未公开的 opt-level 1 下成立。

### 合并协议偏离登记（第四轮；不影响各合并树与验收树的一致性）
1. #119 合并提交标题未以 `[skip ci]` 结尾（正文含有，未触发 Actions）。
2. #120 合并提交使用默认标题、无 `[skip ci]`，触发 Actions "Rust CI"（2026-10-05，success）。
3. #121 以 squash 单父提交合并（9f0c402），标题被追加 "(#121)"，Co-Authored-By 丢失；树等于验收 head d0958ac 的树。
4. #122 以旧公开 head 0e8ff93b 三方合并、默认标题，触发 Actions（2026-10-05，success）；合并树等于本地叠放验收 head c2f89f42 的树，但第四轮状态表记的是公开 head。
5. #123、#124 以 rebase 单父提交合并（618ad83、5295769），树等于验收 head；#123 提交丢失 Co-Authored-By。
6. 其余 38 项均为 `--merge` 两父合并、标题以 `[skip ci]` 结尾、head 已叠放。
- 其它过程偏差：第四轮清单缺 #120/#121/#122/#124 专节；2 个证据文件（AG076 全量日志、AG079 审阅交付 JSON）当前哈希在任何记录中都找不到（证据保管链轻微断点）；第四轮收尾报告称工作树/分支已清理，与现场不符（37 个 worktree、34 个本地分支、3 个远端分支残留，第五轮已导出两处未提交改动为补丁后清理）；可再生测试二进制被压缩归档（5.6 GiB）与 10–42 MB 的 JSON 回执导致磁盘耗尽，已由用户与第五轮主控回收。

### 已核实缺陷与第五轮首批返修卡（均为失败关闭或闸门缺陷，未发现放行坏数据的路径）
- AG-095（R1，#140，E05/E13）：外部宿主完整批次授权的 grader 回执摘要写入用 typed 声明序、校验用 serde_json::Value 排序键，两种编码的 SHA-256 必不相等，RegisteredExternalHost 的授权签发必然 Forbidden，唯一晋级路径在 main 不可达；无成功路径测试。
- AG-096（R2，#125/#113 线，E08/§11）：撤销清理解析历史 bound_model_request 行失败时以 Err 回滚整个命名空间的清理事务且不记 last_error，一条坏历史行即可阻塞该命名空间的清理。
- AG-097（R3，#119/#104，E04/§11）：缓存撤销闸位于 execution 关闭之前，已结算未关闭的调用在来源撤销后永不关闭，整个计费根被 RootConcurrencyLimit 卡住且无运维出口。
- AG-098（R4，#129，E00）：CI 的 Python 单测步骤用 discover 会把 scripts/test_mcp_stdio.py 的 47 个用例一并发现，无二进制时 AttributeError，CI 按构造即红（#129 之后全部提交 [skip ci]，CI 从未运行）；迁移 0007 校验和未钉入 CI。
- AG-099（R5，#140，E00/E09）：上节 7 个测试二进制的栈溢出；只做结构性改动（拆分/Box::pin step 路径上的大 async 函数），不改 profile、不设 RUST_MIN_STACK、不改测试。
- 待裁决（Medium，裁决后再派）：无候选记录即签发 legacy 票据的 bound/unbound 双模式；v2 完整批次永不输出 Regressed 且理由文本陈述未做的 UCB 检查；并发冲突对 call id 的永久拒绝；派发与回放的撤销规则不一致；约 3.5k 行无生产调用方的模块（E16 typed 预算闸、导入 fixture、依赖完整性）需接线或登记为 library-only；索引 command/exit_code 为占位而非实际执行记录；公开台账中的本机绝对路径清洗；0007 之前备份无升级路径；README 缺 `review` 子命令说明。
- 待用户：前端 status-character 是否进入 CI；本地 5.6 GiB JSON 回执是否保留。

### 判定
第四轮合并内容可追溯且结构一致，但 main fa570e0 不满足文档化的测试闸门，并含已核实的 High 缺陷；本台账不声称第四轮"验收通过"。上述返修卡的验收以文档化环境的八项闸门全 0 与至少两个主控反例为准，合并后在本台账登记 merged_sha 与树核对结果。

## 2026-10-11 第五轮首批合并登记（截至 PR #148）

第五轮由两个 Claude 主控窗口并行：一个窗口负责第四轮缺陷 R1–R5 的返修（AG-095–AG-101，另含 AG-105–AG-109），本窗口负责台账、索引诚实字段、恢复升级与公平等待修复（AG-094/AG-102/AG-103/AG-104/AG-110/AG-111）。两窗口共用磁盘闸门（任一编译前空闲 ≥ 8 GiB、同一时刻只跑一个构建、全工作区测试互斥）。本节登记截至 #148 的合并事实；索引记录（reports/support-scope.json）对 #144/#145/#146 的登记按 AG-102 引入的机器字段规则执行（pr > 141 的记录带 `execution` 对象，记录级 `command/exit_code/log_path` 只描述声明的单目标）。

### 合并协议与授权
- 用户于 2026-10-11 13:35Z 与 15:25Z 指令："交付通过后 admin 批准合并"。据此，验收 7.1–7.5 全部通过的 PR 由主控执行 `gh pr merge <N> --merge --match-head-commit <验收 head> --subject "Merge AG-xxx <题> (#N) [skip ci]" --admin`；合并后核对 merged 树与验收 head 树相等。#142/#144/#145/#146 由返修窗口合并，#148 由本窗口合并，均为 admin 合并、两父提交、树相等。
- 文档化验收环境：默认 profile（opt-level 0）、默认测试线程栈、无 RUST_MIN_STACK、`--locked`、不 `--offline`、每次验收使用全新 target。#146 之前 main 的全工作区测试在该环境下有 7 个 evo-engine 测试二进制栈溢出（#140 引入），各 PR 的验收如实记录为 test 退出码 101 + 其余二进制全部通过；#146 合并后首次全绿。

### 逐任务摘要
- **#142 AG-098（CI Python 闸门、MCP stdio 步骤、迁移 0007 校验和）**：head 2236ad1944138642dd7f0ba4fadb3c613adec07d → merged e727d1c65d87de36d85983e606c34b725f5ff94e（2026-10-11T13:46:05Z）。无 Rust 改动；Python/CI 门在 head 上全部退出 0（discover Ran 141 OK skipped=47；MCP stdio 47/47）；Rust 测试门沿用 fa570e0 的全量结果（crates/、apps/、Cargo.* 与 fa570e0 完全相同），未声称本 PR 新跑。按先例（#98/#102/#107/#112/#115/#120/#129/#139/#141）不进索引记录。
- **#144 AG-097（已结算调用在拒绝撤销缓存内容前先记录恢复关闭）**：head 98431d59d765f0c16a5b736147de4735d2adbe2b（交付 29d35927 于 fa570e0，叠放到 e727d1c6 补丁字节一致）→ merged 184764302e64ca3871fc68103bd07abec1125918（14:52:30Z）。全量：fmt/clippy 0，test 101（128 二进制 1630 通过 0 失败 = 父 1627 + 新 3，7 个已知溢出二进制不变），build/smokes/checker/unittest 0；主控反例 2 个 head 通过、父 e727d1c6 失败。索引记录 E04.ag097_broker_close_before_refuse（V007/V008/V086/V096）。
- **#145 AG-096（撤销清理把不可读的历史预算请求行隔离为未知作用域而非回滚命名空间）**：head 4fd5701ad6677c0c441fd376eec953e001fc7012（叠放到 18476430 补丁字节一致）→ merged 7d39ef7350615be82d698d6914d03856fa0df97c（16:06:26Z）。全量：fmt/clippy 0，test 101（129 二进制 1639 通过 0 失败 = 父 1630 + 新 9，溢出集合不变），其余 0；反例 2 个 head 通过、基线 18476430 失败。索引记录 E08.ag096_unreadable_bound_request_isolated（V017/V018/V075/V096）。
- **#146 AG-099（装箱 step 路径 async future，7 个 evo-engine 测试二进制在默认测试栈内完成）**：head 45377b9d78c373fbd68b89221ba138e9eed114ab（叠放到 7d39ef73）→ merged 98776f1717b37a47e4a7fcc4349ff0faff68ce1b。全量首次全绿：fmt/clippy 0，test 0（136 二进制 1741 通过 0 失败，溢出 0），其余 0；反例：7 个此前溢出目标在 head 全部通过（102 项），在基线 7d39ef73 全部溢出。索引记录 E00.ag099_step_future_default_test_stack（V001/V080；test_entry 取 step_terminal_classes_v42）。
- **#148 AG-102（索引机器字段：声明目标与实际执行证据分离）**：base e727d1c → head 971d4eb3d9abd79aeb02374d4393c6da67e00cc0（叠放到 98776f17 补丁字节一致）→ merged 1ef0b2163fe9b6c46c9d49b558e6e25c0e13ab12（16:52:06Z）。改动仅 4 文件（reports/support-scope.json、scripts/check_support_scope.py、scripts/test_check_support_scope.py、crates/evo-core/tests/support_scope.rs）：顶层 `field_semantics` 钉死 command/exit_code/log_path/execution/production_consumer 的含义；pr > 141 记录必须带 `execution{runner, command, scope, exit_code, evidence}`；`production_consumer=none_library_only` 标注 #125/#128/#130/#131 四条记录（截至 e727d1c 无 engine/http/mcp/apps 生产调用方）；追溯回填 13 处（#103→V002；#105→V033/V035；#106→V010–V013/V084/V085/V096/V097；#114+#117→V067）；92 条旧记录除上述四条外字节不变。全量：8 项退出码全 0，136 二进制 1748 通过（父 1741 + 7），Python discover 160 OK；索引篡改探针 6 例在 head 全部被拒、其中 2 例在父树被接受（真反例）。按先例不进索引记录。登记的偏差：Python 检查器按所属任务规则拒绝 AG-058 对 V001/V002/V017/V032/V037 的引用而 Rust 镜像仅查引用存在，两者在该规则上不一致，未在本 PR 修复。
- 清理：各 PR 验收 target 已删除；分支删除按用户命令清单执行。

## 2026-10-11 第五轮第二批合并登记（截至 PR #155）

本节登记 #143 之后至 #155 的合并事实。验收环境与合并协议同首批（文档化环境、全新 target、admin 合并、树相等核对）。两窗口共用构建互斥锁与 1.5 GiB 低磁盘看门狗；本窗口编译门为空闲 ≥ 8 GiB，返修窗口冷 target ≥ 6.5 GiB、暖 target 上的探针运行 ≥ 4.0 GiB。索引记录按 AG-102 机器字段规则登记：pr > 141 的记录带 `execution`（返修窗口验收时字面执行的命令、真实退出码与日志）；记录级 `command/exit_code/log_path` 只描述声明的单目标，由本台账 PR 的验收全量覆盖并以逐目标摘录日志为证。

### 逐任务摘要
- **#143 AG-094（台账 + 索引登记）**：base e727d1c → 最终叠放 1706cb04，验收 head 35826bbb0035aab5e91f46c8acf5f77f1b315a85 → merged ab2f41003483aec2c2067cd1fed65b119bd0da3b（17:49:01Z）。折入第四轮二次验收结论与第五轮首批队列；登记 #144/#145/#146（E04/E08/E00 记录，各带 execution 与主控输入回执）、support_scope.rs 钉值、顶表行。全量 137 二进制 1751 通过（= 父）。索引篡改探针 5 例被拒；登记一处 Python 检查器缺口（merged_sha 等于自身 source_sha 不被 Python 拒绝、由 Rust 钉值表拒绝），列为后续小卡。台账 PR 按先例不进索引。
- **#147 AG-095（独立 grader 回执的规范摘要绑定外部宿主权威）**：head d73acb6f（叠放 1ef0b216，补丁字节一致）→ merged 1706cb04（17:24:52Z）。全量 137 二进制 1751 通过（父 1748 + 3）；反例 2 个 head 通过、基线 1ef0b216 失败。索引记录 E05.ag095_canonical_grader_receipt_digest（V010/V011/V012/V013/V084/V085）。
- **#149 AG-105（已结算 bound 调用在拒绝重放前先关闭执行）**：首版验收时主控反例 P2 在 head 失败，退回实施者修复一处（预读身份检查对脱敏请求跳过指纹比较）；修复版 head 9862bbd7（叠放 94076d78）→ merged 7dc89bff。全量 138 二进制 1762 通过（父 1756 + 6）；反例 2 个 head 通过、基线失败。索引记录 E04.ag105_broker_bound_recovery_close_v42（V096）。
- **#150 AG-108（EditCompiled 审阅核验生产者的 input_digest 规则）**：head f8539172（叠放 ab2f4100）→ merged 94076d78（18:35:46Z）。全量 137 二进制 1756 通过（父 1751 + 5）；反例 2 个 head 通过、基线失败。索引记录 E07.ag108_candidate_review（V070）。
- **#151 AG-107（旧预算门按请求闭包判断丢失引用的调用，无法核验则关闭）**：head 7cc0e962（叠放 7dc89bff）→ merged 63113ac9。全量 139 二进制 1771 通过（父 1762 + 9）；反例 2 个 head 通过、基线失败。索引记录 E08.ag107_budget_ref_missing_fail_closed_v42（V096；scope 同时引 E03 L1229）。
- **#152 AG-101（v2 完整批次回归仅在 UCB 低于零时确认）**：head 78075a11（叠放 63113ac9）→ merged f355ec38。全量 139 二进制 1773 通过（父 1771 + 2，另修正 1 个既有用例）；反例 2 个 head 通过、基线失败。索引记录 E05.ag101_streaming_verdicts_v42（V010/V011/V012/V084）。
- **#153 AG-109（在绑定票据上签发与读取外部宿主权威）**：head e573eaa3（叠放 f355ec38）→ merged 42decd1a。全量 139 二进制 1778 通过（父 1773 + 5）；反例 2 个 head 通过、基线失败。索引记录 E05.ag109_external_host_authority_bound_ticket（V010/V012/V084/V085）；其声明目标与 #147 同为 formal_external_host_authority_v42，两条记录各自指向本次验收日志的独立摘录（.ag095 / .ag109）。
- **#154 AG-106（serve 启动时关闭崩溃丢失执行关闭的已结算预算调用）**：head 71cae0cf（叠放 42decd1a，补丁字节一致）→ merged aece7299（23:24:51Z）。全量 140 二进制 1785 通过（父 1778 + 7）；反例 2 个 head 通过、基线因新 API 不存在不编译（基线证据）。索引记录 E08.ag106_budget_startup_recovery_v42（V096）。
- **#155 AG-110（公开台账与测试助手去本机路径、README 子命令）**：base ab2f4100 → 叠放 aece7299，验收 head d0cf8773 → merged ac57058e（23:59:55Z）。台账 444 处路径替换（字节级两种实现一致）、privacy_utf8_boundary_v42.rs 的硬编码路径守卫改写为"绝对路径且位于仓库 out/ 树内"、README.md/README.zh-CN.md 列出 review/serve/mcp/manage 的 clap 参数与默认值。全量 140 二进制 1785 通过（= 父）；探针：台账 `/Users/<login>` 计数 0、两份 README 含四个子命令、测试文件无本机路径。按先例不进索引。

### 更正与说明
- 本台账 2026-10-01 AG-052 条目中"v2 保留 Improved/Invalid/原 Inconclusive，negative_lcb 不称确认退化"描述的是 #100 合并时的行为；自 #152（AG-101）起，complete_batch v2 在 UCB 低于零时确认 regressed，其余情形为 Inconclusive（原因 regression_not_confirmed_ucb_boundary_not_crossed）。原句保留为历史记录，不改写。
- 索引记录 E05.ag052_versioned_batch_verdicts 的 actual_result 写明 "streaming_verdicts_v42 10 passed"，是 #100 验收时该测试文件的用例数，为那次运行的事实，不随后续改动更新；#152 之后该文件为 12 个用例，由 E05.ag101_streaming_verdicts_v42 记录承载。
- #142、#143、#148、#155 为非产品 PR，按先例只在台账登记。

## 2026-10-12 第六阶段登记（PR #161–#163）

本节登记第六阶段合并的三个产品 PR。第六阶段与第五轮收尾并行启动，等第五轮编译序列结束（main 8f4fe54b，#160）后才起 cargo。用户中途下达指令"第六阶段的在途任务完成后不新增"，因此本阶段只完成在途三项，并以本台账 PR 收尾。

验收环境与合并协议同第五轮：
- 文档化环境：默认 profile、默认测试线程栈；每个 PR 用全新 target 跑完整八门。
- 主控反例至少两个，head 通过、父树失败；只加测试的 PR 以"head 通过、含缺陷的旧树失败"为检出证据。
- L 档任务验收前加一次只读审查。
- admin 合并只用 `--merge`、固定验收 head；合并后核对 merged 树与验收 head 树相等、两父提交。

索引记录按 AG-102 机器字段规则登记：记录级 `command/exit_code/log_path` 描述声明的单目标，以该 PR 验收全量的逐目标摘录为证；`execution` 指向该 PR 的验收全量日志。

### 逐任务摘要
- **#161 AG-116（生产路径写出的 EditCompiled 事实经审阅视图呈现精确 diff，只加测试）**
  - base 8f4fe54b，验收 head 2fc3187dc1e1fdcdc9caee3b635157f4d23529b3 → merged 5401effdfadf048dd4537fdf1e1ca843b9482ac3（2026-10-12 07:01:37Z）。
  - 新测试从真实优化步骤写出的事实一路审到 review_stored：输入摘要等于共享规则；篡改一个字节被拒（material_invalid）；StepPrepared 完整绑定。
  - 从 tests/optimization.rs 复制的夹具种子水位改为 hash：审阅要求 64 位 hex，生产写入方写的都是 hash。
  - 全量 145 二进制 1813 通过（父 144/1810，新增 1 二进制 3 测试）。
  - 主控反例 2 个，在 head 上通过；在 AG-108 之前的树（7d39ef73）上均失败（material_invalid），证明该测试能检出 AG-108 所修的缺陷。
  - 索引记录 E06.ag116_edit_compiled_produce_review_v42（V070）。
  - 不声称：真实模型、CLI/HTTP 审阅链路、其他阶段事实。
- **#162 AG-117（撤销清理按登记的预算来源引用判定调用闭包）**
  - base 5401effd，验收 head 92f12e1642bd0f5c07ad6f4e431635a46f1671d8 → merged 5718bf5c0389588781b8dd2e76518da7daab6fd5（2026-10-12 07:36:33Z）。
  - 修复前：命名空间里只要有一条 rsia.formal_task_request.v1 预算调用（其请求体不含来源闭包），之后的每个撤销清理作业都停在 Failed，Admin 重试也无法完成；依赖被撤销来源的 formal 请求体也从不脱敏。
  - 现在清理与派发/结算闸门同序：先读登记的 budget-ref，没有 ref 才读请求体。
    - 命中时沿用既有脱敏路径，记账与 ref 保留；不命中零写入。
    - ref 不可解码时只隔离该行。闸门侧仍返回 Internal，这一差异是有意的。
  - 全量 146 二进制 1824 通过（父 145/1813，新增 1 二进制 11 测试）。实施者基线钉住 4 过 7 败；只读审查无阻断。
  - 主控反例 2 个，在 head 上通过、在父树上失败：分页扫描只脱敏依赖行并完成；同一命名空间先后两次撤销均能完成。
  - 索引记录 E08.ag117_formal_call_revoke_cleanup_v42（V017/V018）。
  - 不声称：空闭包 formal 调用、历史上已 Failed 作业的自动恢复（需 Admin 重试）、E16 路径。
  - 已知可观察效果：正式授权路径读到已脱敏的 formal 请求工件时返回 Forbidden。
- **#163 AG-118（正式报告终结后到达的迟到评分/执行回执保留审计、不改写报告）**
  - base 5718bf5c，验收 head ad9660c2ec45d67260a2fd124c4b3456f63c99fd（两个提交）→ merged 4d1fd6cb43fbe64db81177d336b6dae3f061ca98（2026-10-12 08:40:25Z）。
  - 修复前：
    - CompleteBatch/Invalid 报告终结后到达的迟到评分会重写票据，已持久的报告从此在读侧永久 Conflict。
    - Invalid 之后的迟到评分如果出错，会再次 invalidate 并追加原因。
    - EarlyStopped/Invalid 之后的迟到执行回执同样会重写票据。
  - 现在：
    - 报告终结后的迟到评分只存回执、记审计；票据、边与报告逐字节不变。
    - 终结后评分出错只记审计、返回原错误，原因保持不变（§6.6 L705）。
    - Invalid 之后的迟到执行回执只存回执与输出，并记审计。
    - EarlyStopped 之后的迟到执行回执沿 AG-091 模式记到票据上并重派生报告：证书不变，报告 owner 仍是评估者。
  - 经授权，以更强的断言替换了 formal_candidate_dependency_binding_v42.rs 中一处钉住缺陷行为的旧断言。这不是删断言凑通过（L1189）；该文件在父树上为 27/0。
  - 首次交付把新测试放在 64 MiB 线程上运行，主控未接受。定位结果是测试 helper 嵌套过深；改为让 helper 返回装箱 future 后，在默认测试栈下通过。这一改动以追加提交完成，产品代码没有因此改动。
  - 全量 147 二进制 1840 通过（父 146/1824，新增 1 二进制 16 测试）。实施者在默认栈下钉住基线 1 过 15 败；只读审查无阻断；主控反例 3 个，在 head 上通过、在父树上失败。
  - 索引记录 E05.ag118_late_receipts_after_final_report_v42（V085）。
  - 不声称：已 Conflict 历史报告的修复或迁移、只审计迟到回执的审阅投影、真实 grader 或宿主。

### 第六阶段总结
- **合并的 PR**：#161 AG-116、#162 AG-117、#163 AG-118，以及本台账 PR（AG-123，按先例不进索引）。
- **产品主线**：4d1fd6cb43fbe64db81177d336b6dae3f061ca98（#163 合并提交）。147 个测试二进制、1840 通过、0 失败；fmt、clippy、build、两个 smoke、checker、unittest 全为 0。
- **裁决与侦察**（只有本地记录）：
  - 返修窗口 7 项候选的处理：formal_task_request.v1 不在共享闭包解析器内（候选 1）判为已合规；AG-106 启动并发窗口（候选 6）判为在 serve 进程内不可达；produce→review 端到端测试（候选 5）落为 AG-116；其余几项由主控裁决后列入下一阶段队列。
  - 侦察另外发现了 AG-117、AG-118 两个缺陷，本阶段已修复。
- **未派发，转下一阶段**：
  - AG-119：预算闸按上游闭包判定撤销（当前仅夹具可达）。
  - AG-120：同一预算调用至多一张执行回执。
  - AG-121：脱敏后 bound 守卫与重放身份核验不被削弱。
  - AG-122：豁免附着边构成的环，只在其余边全为严格校验边时接受。
  - AG-114：检查器拒绝 merged_sha 等于自身 source_sha；Rust 侧 V/B/U/K/SO 引用按所属任务检查。
  - AG-111：台账尾部压缩。
  - 审查与侦察登记的若干加固项。

## 2026-10-12 第五轮收尾登记（截至 PR #159）

本节登记 #155 之后的合并事实并收口第五轮。用户 2026-10-12 00:10Z 指令：第五轮不再新增任务，完成在途项后收尾；第六阶段改由另一主控窗口执行，Claude 负责二次验收与修正。验收环境与合并协议不变（文档化环境：默认 profile、默认测试线程栈；全新 target；admin 合并、树相等核对；两窗口共用构建互斥锁、≥ 8 GiB 编译门与 1.5 GiB 低磁盘看门狗）。索引记录按 AG-102 机器字段规则登记；记录级 `command/exit_code/log_path` 只描述声明的单目标，由本台账 PR 的验收全量覆盖并以逐目标摘录日志为证。

### 逐任务摘要
- **#156 AG-113（台账 + 索引登记第二批）**：base ac57058e → 叠放 647c7ce8，验收 head 4042fcbc28f4f5b1b782ed1b9c25d58ac1396669 → merged c39cbd626faf4a4c9c0dd1dd104f67282b915f45（2026-10-12 01:58:29Z）。登记 #147/#149–#154 七条索引记录（各带 execution 与主控输入回执）、support_scope.rs 钉值、顶表行与第二批摘要。全量 140 二进制 1789 通过（= 父）；索引篡改探针对新记录全部被拒。台账 PR 按先例不进索引。
- **#157 AG-100（任意 control schema（v1/v2）下票据签发绑定冻结候选依赖，无候选记录即拒绝）**：head afa1928e（叠放 ac57058e，内容逐行一致）→ merged 647c7ce8（2026-10-12 01:01:56Z，返修窗口验收、admin 合并）。全量 140 二进制 1789 通过（父 1785 + 4）；反例 2 个 head 通过、基线失败（无候选记录仍签发 / 旧票据 Blocked 而非零写入）。索引记录 E05.ag100_formal_candidate_dependency_binding_v42（V010/V012/V084/V085），由本台账 PR 登记（#157 合并时其条目尚未到达，故未进 #156）。
- **#158 AG-103（公平等待只计核心过滤后的合法机会，在线探索与回放共用一个核心过滤器）**：base 98776f17 → 叠放 c39cbd62，验收 head d8a71327018e3995f786bed3dd93e852c858e3ce → merged 4036c0d2d11a01ddcdcb2940d0b00f0f00975656（2026-10-12 02:51:38Z）。裁决 D2（方案 L797 选择前剔除非法/预算不足动作；L805 公平等待按已完成的决策轮推进）：等待只对"已完成决策轮中合法且被跳过"的机会累计；evo-core 抽出 `legal_candidates`，decide_elastic 只排序其输出，决策不变；replay/exploration 共用该过滤。全量 143 二进制 1805 通过（父 140/1789 + 3 二进制 16 测试）；主控反例 2 个（P1 无费用根不累计 waits；P2 深度上限加深不累计 waits）head 通过、父树失败。索引记录 E09.ag103_fairness_waits_legal_set_v42（V020/V022/V025/V081/V086，目标 fairness_waits_legal_set_v42）与 E10.ag103_replay_fairness_waits_legal_set_v42（V020/V022/V023/V025/V081/V086，目标 replay_fairness_waits_v42）。
- **#159 AG-104（restore 在严格迁移前缀上升级备份后再发布）**：base e727d1c → 叠放 4036c0d2，验收 head a2705252894c61aaa1972f0596a279e2f36c2ab5 → merged a78a1e60a4298cd3ca57598a7aba7c5c84a8a721（2026-10-12 05:43:11Z）。scripts/restore_backup.py 接受清单与数据库账本为当前迁移集严格前缀（0001-0005 或 0001-0006，全部成功且校验和匹配）的备份，在私有恢复副本内按 sqlx 0.8.6 的方式补跑缺失迁移后再做锚点比对、增量重放与发布；缺口/未知版本/校验和变化/未成功行/0005 之前的前缀/清单与库不一致仍隔离；可信锚点与启动门不变。全量 144 二进制 1810 通过（父 143/1805 + 1 二进制 5 测试）；主控反例 2 个 head 通过、父树失败。索引记录 E08.ag104_restore_prefix_migration_upgrade_v42（V017/V018/V038/V081/V082，目标 restore_prefix_upgrade_v42）。

### 第五轮总结
- 合并的 PR（按合并顺序）：#142 AG-098、#144 AG-097、#145 AG-096、#146 AG-099、#148 AG-102、#147 AG-095、#143 AG-094、#150 AG-108、#149 AG-105、#151 AG-107、#152 AG-101、#153 AG-109、#154 AG-106、#155 AG-110、#157 AG-100、#156 AG-113、#158 AG-103、#159 AG-104，以及本收尾台账 PR（AG-115）。每个 PR 的验收 head / merged_sha 见本台账顶表与各节摘要。
- 第五轮末次文档化环境全量（第五轮产品主线 = a78a1e60a4298cd3ca57598a7aba7c5c84a8a721（#159 合并提交，本收尾 PR 的父；本 PR 合并后的 main 为其 merge commit，见 PR 正文验收段））：144 个测试二进制、1810 通过、0 失败（AG-104 验收 head a2705252，与 a78a1e60 同树；本 PR 自身的验收全量见其 PR 正文验收段）；fmt、clippy、build、两 smoke、checker、unittest 全 0；support checker structure_valid、errors 0。
- 修复的第四轮缺陷：R1–R5（AG-095–AG-101）与 AG-105–AG-109 已全部合并；main 测试闸门自 #146（AG-099）起在默认栈下绿。
- 未派发（转第六阶段）：AG-111 台账尾部压缩为逐任务摘要；AG-114 Python 检查器拒绝 merged_sha 等于自身 source_sha 并与 Rust 钉值表共享合并事实；Python/Rust 校验器对 V 引用所属任务规则的统一；返修窗口 7 项候选。
- 残留与待用户：(1) 待用户：user-commands.md 的远端分支删除（ag-102/094/110/113/103/104）与清理节；仓外磁盘占用（~/.codex 历史库与 rollout、Docker、Codex worktree 构建产物）；本地 5.6 GiB JSON 回执是否保留；queue-r5.md 待用户裁决项（E12 选题受资产影响规则、GB16 其余未定义项、GA10 typed HostSurface 合同、GA21 取消语义、GC6 失败优先级语义、继承第三/四轮项）。(2) 已知缺口：Python 检查器不拒绝 merged_sha 等于自身 source_sha（Rust 钉值表拒）→ AG-114；Python 与 Rust 对 V 引用所属任务规则不一致；AG-111 台账尾部压缩未做；AG-103 两条未声明边界（新可用动作入零、末轮后失格动作保留等待项）；AG-104 不声明 0005 前备份、磁盘耗尽与运维升级自动化。(3) 环境：本机 rustup minimal profile 下 rust-objcopy strip 警告（非 lint）；两窗口共用的 ≥8 GiB 门与 1.5 GiB 看门狗记录于本地台账。

### 第六阶段交接
- 第六阶段自 2026-10-12 01:1xZ 由另一 Claude 主控窗口（Opus 5.5）按本地 claude-start-prompt-r6.md 执行，编号自 AG-116；与第五轮收尾并行期间共用构建互斥锁与编译门，第五轮编译序列结束前不起 cargo。第五轮的 Claude 窗口在第六阶段结束后做二次验收与修正。

## 2026-10-11 第四轮在途收尾与最终证据索引

按用户最后指令停止新增任务，仅接收原在途交付、必要关联返修和本次收尾台账。原在途产品与透明3D状态小人前端均已实际合并，当前产品主线为 f6904dcd2d4f3b46b3311886fd4dcecd8a2f711c。第四轮 #98 至 #140 已合并；本次三文件台账自身以此主线为父，仍须独立全量与两项原索引反例通过后按固定 head 合并。

新登记20个已实际合并的 Rust 软件子范围，索引由72增至92条。原72对象的正文、顺序和版本逐字保留，顶层仍为唯一 v4.2 方案及既有谱系。AG056 CI、AG078 Python费用报告与前端 PR139 记入台账，未挤入单目标 Rust 索引文法。最新 AG093 的实际固定验收与规定至少两项 ROOT 独立反例见下表及机械折入记录；未获验收信用的诊断如实保留。较早“未合并”“待授权”“外部工具”与失败源的记录是历史事件，以本节和实际 Git/PR 状态判断本次在途收尾。没有外部工具独占的当前尾项，定时 rsiagent-30 已取消。

| PR | 原在途任务 | 验收 source_sha | 实际 merged_sha | 索引子范围 |
|---|---|---|---|---|
| [#118](https://github.com/acosmi/RSIAgent/pull/118) | AG069 | 9edf57e896a1b13fc7f57cf5a6392657a23da9a9 | 5ee01d1e2ebb2401fe94530ff286801239940259 | E08 |
| [#119](https://github.com/acosmi/RSIAgent/pull/119) | AG073 | b67ca370b0dca0163465bb8037bf940c2ae38b79 | 06fb166b006e5479a80e04dc14998109c30b0d42 | E08 |
| [#121](https://github.com/acosmi/RSIAgent/pull/121) | AG079 | d0958acfdd234007056aec09277cca676b87dd52 | 9f0c40215a2d7a67ba66e5427cee71ca5228bde1 | E03 |
| [#122](https://github.com/acosmi/RSIAgent/pull/122) | AG081 | c2f89f42ebb06703ee70af96fd5cf7e56082c722 | 90d3dcbef822b17abaffffbcc866b863336c4024 | E13 |
| [#123](https://github.com/acosmi/RSIAgent/pull/123) | AG080 | 4ed8d9d886b8e5c1f45d2477478de53fe5ee6348 | 618ad834ff73e878898fb64681a4efc323b84ac0 | E05 |
| [#124](https://github.com/acosmi/RSIAgent/pull/124) | AG082 | cd19f84b85712447820b893a8c72bf7d011cd0e7 | 5295769892af37988d6bc739bdfb49a834a3f7aa | E04 |
| [#125](https://github.com/acosmi/RSIAgent/pull/125) | AG077 | 26d3fe400ee1aad36082081eda6c60f3ee1a17f7 | 349879148c1f31fd1ebbee639aeff334a3762855 | E04 |
| [#126](https://github.com/acosmi/RSIAgent/pull/126) | AG084 | 3c5ae3d6dacf74b7b91a7bf5527226c561ca688c | ea02f116fe2172dcba621a60bf9b141237e1ba45 | E06 |
| [#127](https://github.com/acosmi/RSIAgent/pull/127) | AG074 | 313598036461304fe1b081e91115bdbd2c779752 | aa51b1102e1886cbaf7b025d37b98ca30b6e2d3d | E09 |
| [#128](https://github.com/acosmi/RSIAgent/pull/128) | AG076 | 1355e95f598bf7d1f490d6d1d295a1e29c855c0c | 1177c91931689794a7d28b614adc01d3a0435e46 | E16.1 |
| [#130](https://github.com/acosmi/RSIAgent/pull/130) | AG070 | 887d97a345e947c754dd01c0fd0e1dd15b635d5c | 1132b6b4b6e0016540339ba65219daf1ceb35fb8 | E16.1 |
| [#131](https://github.com/acosmi/RSIAgent/pull/131) | AG075 | 8a92e578527b6334b5fb776abf46c5bc9a88e4b7 | aa831fba2d09d0ef2d0755c3d7a08971b8f6667f | E08 |
| [#132](https://github.com/acosmi/RSIAgent/pull/132) | AG086 | 908eb85b33d590889a75885c9d9b20c3dad927b8 | 77152d77c96f497ebf2c0123e985f0832fa3cbd0 | E16.6 |
| [#133](https://github.com/acosmi/RSIAgent/pull/133) | AG087 | fde3dd4b91e9f6f68f93f7b8fe54ec2e74a08fe8 | 8cbf82cb9a715a8b28070c6a7823b16093865425 | E05 |
| [#134](https://github.com/acosmi/RSIAgent/pull/134) | AG088 | e538b9664fb9199fead8f2298a27e38da686cd7b | bd20099a01c64b87b9a75707a95c785486bb4fda | E05 |
| [#135](https://github.com/acosmi/RSIAgent/pull/135) | AG089 | 6040bf338e3a9a929f036c3e57891729e775f6fe | c9aa71e7516e369672b0e5127e675d72eb199304 | E05 |
| [#136](https://github.com/acosmi/RSIAgent/pull/136) | AG090 | a9944434b45f4c59c46329a69e5d9c8be4a60ac8 | 1f1b593d5475043e707e17195222d82f2c5281eb | E05 |
| [#137](https://github.com/acosmi/RSIAgent/pull/137) | AG091 | 730f242cd4fe076c4d751e3f823cc8a9385fbb93 | 27df8c2cfccbefa6544b55b697c10abbb6c0a61c | E05 |
| [#138](https://github.com/acosmi/RSIAgent/pull/138) | AG092 | 39d2d865c203ca45dd278efa591c934abffb0165 | 77ef86294b7760fc7495d12d4029ca8a3567bc5e | E02 |
| [#140](https://github.com/acosmi/RSIAgent/pull/140) | AG093 | 49806d7faf703aab7e7e467c4bd5518969a24e55 | f6904dcd2d4f3b46b3311886fd4dcecd8a2f711c | E02 |
| [#144](https://github.com/acosmi/RSIAgent/pull/144) | AG097 | 98431d59d765f0c16a5b736147de4735d2adbe2b | 184764302e64ca3871fc68103bd07abec1125918 | E04 |
| [#145](https://github.com/acosmi/RSIAgent/pull/145) | AG096 | 4fd5701ad6677c0c441fd376eec953e001fc7012 | 7d39ef7350615be82d698d6914d03856fa0df97c | E08 |
| [#146](https://github.com/acosmi/RSIAgent/pull/146) | AG099 | 45377b9d78c373fbd68b89221ba138e9eed114ab | 98776f1717b37a47e4a7fcc4349ff0faff68ce1b | E00 |
| [#147](https://github.com/acosmi/RSIAgent/pull/147) | AG095 | d73acb6f4ddce9bbdca0702eab783e54cf41ad90 | 1706cb04049c0306676a17fd34fd021717c5669b | E05 |
| [#149](https://github.com/acosmi/RSIAgent/pull/149) | AG105 | 9862bbd7c6e1945ea0f7b6431db4dcab7a4db346 | 7dc89bffc49d8fe4c5d97e7ccd56d3d0ad597f1c | E04 |
| [#150](https://github.com/acosmi/RSIAgent/pull/150) | AG108 | f853917248fb025abe3144dc79420fa695644517 | 94076d789f40347ab8263176044f041824c061fb | E07 |
| [#151](https://github.com/acosmi/RSIAgent/pull/151) | AG107 | 7cc0e962936956ddab4acd5d6a25bf8cf14d336f | 63113ac95b01f78863da0f75c5a93e5bfa2dd9d8 | E08 |
| [#152](https://github.com/acosmi/RSIAgent/pull/152) | AG101 | 78075a1101db12e5f3591622f0499ecf2b7b66bf | f355ec389513685097e2c4d85dfd0c062e29e915 | E05 |
| [#153](https://github.com/acosmi/RSIAgent/pull/153) | AG109 | e573eaa3a8cea90932f806ca455bf8a3963e1a3d | 42decd1a17585c366b249104b0bbea700361d003 | E05 |
| [#154](https://github.com/acosmi/RSIAgent/pull/154) | AG106 | 71cae0cfd8919f523514d382e0b966171c5e3f97 | aece7299d0078e5704bd74ed3ae04e828f3e2c6d | E08 |
| [#157](https://github.com/acosmi/RSIAgent/pull/157) | AG100 | afa1928e7f9367fd125bfac40cd8115dbe7a17ac | 647c7ce8361215632d62a42a8bf87d9df2bf3c2d | E05 |
| [#158](https://github.com/acosmi/RSIAgent/pull/158) | AG103 | d8a71327018e3995f786bed3dd93e852c858e3ce | 4036c0d2d11a01ddcdcb2940d0b00f0f00975656 | E09/E10 |
| [#159](https://github.com/acosmi/RSIAgent/pull/159) | AG104 | a2705252894c61aaa1972f0596a279e2f36c2ab5 | a78a1e60a4298cd3ca57598a7aba7c5c84a8a721 | E08 |
| [#161](https://github.com/acosmi/RSIAgent/pull/161) | AG116 | 2fc3187dc1e1fdcdc9caee3b635157f4d23529b3 | 5401effdfadf048dd4537fdf1e1ca843b9482ac3 | E06/E07 |
| [#162](https://github.com/acosmi/RSIAgent/pull/162) | AG117 | 92f12e1642bd0f5c07ad6f4e431635a46f1671d8 | 5718bf5c0389588781b8dd2e76518da7daab6fd5 | E08 |
| [#163](https://github.com/acosmi/RSIAgent/pull/163) | AG118 | ad9660c2ec45d67260a2fd124c4b3456f63c99fd | 4d1fd6cb43fbe64db81177d336b6dae3f061ca98 | E05 |
| [#120](https://github.com/acosmi/RSIAgent/pull/120) | AG078 | a4b1ac77f4b56c2328cff32040370c678066fd59 | d56e2bef5cd552e0055d532f1e1908ed24921dcf | Python预算报告（有限E04/E16.5，不授Rust索引） |
| [#129](https://github.com/acosmi/RSIAgent/pull/129) | AG056 | e991c86eafb40e2bccbd8fe2d1aa3c184b9d92bb | 3ad643ca2a1cf57f4e7bc1c0f032eef2663902b5 | CI/inventory（非Rust本地事实） |
| [#139](https://github.com/acosmi/RSIAgent/pull/139) | 用户委派前端 | f9b7148eecbe1d38553fa4f3b0839b4e609e734f | c04850d702d43aecb52b57c94e2770a4c6c594c1 | 方案外透明状态小人（mock/GPU） |

| E任务 | 默认完整范围状态 | 已验子范围记录数 |
|---|---|---|
| E00 | implemented_not_verified | 1 |
| E01 | blocked | 2 |
| E02 | in_progress | 6 |
| E03 | in_progress | 3 |
| E04 | in_progress | 4 |
| E05 | blocked | 9 |
| E06 | blocked | 2 |
| E07 | in_progress | 5 |
| E08 | in_progress | 14 |
| E09 | in_progress | 11 |
| E10 | in_progress | 4 |
| E11 | in_progress | 1 |
| E12 | in_progress | 3 |
| E13 | in_progress | 6 |
| E14 | in_progress | 3 |
| E15 | planned | 0 |
| E16.1 | in_progress | 3 |
| E16.2 | in_progress | 3 |
| E16.3 | in_progress | 1 |
| E16.4 | blocked | 2 |
| E16.5 | in_progress | 5 |
| E16.6 | in_progress | 2 |
| E16 | in_progress | 0 |
| E17 | in_progress | 1 |
| E18 | in_progress | 1 |

透明状态小人是现有后端状态的只读展示；完整14文件在当前主线经过八门、原 Node10 与 ROOT Node2 验收后按 PR139 合并。原生浏览器12项证据只覆盖原样设计呈现与显式模拟状态，真实后端连接、OS透明叠层和像素等价尚无独立验收。它不是宠物，也不把业务完成显示解释为批准、采用或收益。

本次在途收尾完成不代表整个 v4.2 路线完成。完整 E/V、真实独立 OS principal、provider/生产宿主、真实模型与效果、发布验收继续按第一真源逐范围判断；原 support checker 的 needs_verification 如实保留。旧拒绝 Source、所有未提交源、费用/水位历史及 QA/RAW/SQLite 仍在本地，已结束的自有可再生编译缓存按实际条件清理，不把历史路径退役冒称当前程序存在。

下面原公开台账完整保留；末尾追加上次折入之后的本地事件，仅机械给标题增加两个 #，不重写历史结论。


## 2026-10-03 第四轮第五批证据折入（AG-086）

本批固定基线 main / origin/main 为 ddde63c6ab98a2528258d265999bb902a08b0f55，源码树10725a0c6a86397adfbd415bd81581f399626554。第四轮 PR #98–#117 共20项已经实际合并，其中15项为产品或回归子范围，5项为台账增量。最新产品 AG085 的主控八门全部exit0，Rust 1326通过/0失败、Python检查器48通过；这些是该验收时点的结果。本批台账自身的验收另登记，不能把此前产品全量当成新台账复跑。

| PR | 任务与有限作用域 | 验收 head | 实际 merged_sha |
|---|---|---|---|
| [#114](https://github.com/acosmi/RSIAgent/pull/114) | AG071 已知隐私模式的 UTF-8 安全阻断 | 53dd17677a38abab26d31f1a57778b7fca9694e6 | c0c4dbd3f6f0464c6cb492c0eb298496fd88b9de |
| [#115](https://github.com/acosmi/RSIAgent/pull/115) | AG072 第四批台账与索引 | f689d2f28f7d9064fcd969def0808ac32070917e | 9b9084a328b78d7376247b78cc26e75235a09afa |
| [#116](https://github.com/acosmi/RSIAgent/pull/116) | AG083 纯内存 partition 单次拒绝无部分登记 | 215811ff8848817e72f0e3226b81d57e0fef35ea | 7fa290883136f30fcdc87ff5a4de293aa0714507 |
| [#117](https://github.com/acosmi/RSIAgent/pull/117) | AG085 原 scanner 的 UTF-8 边界与公共拒绝回归 | 9f7038d1d033de500d858b188bbf344086821b1d | ddde63c6ab98a2528258d265999bb902a08b0f55 |

三项新产品记录分别绑定原始1283/48、1314/48、1326/48验收与10/31/12项主测试；三实际合并树各与当场验收树一致。AG071原本地记录中的未外发/未合并是历史状态，后续用户授权、实际推送与合并事件完整保留。AG083实际接受的是215811f，旧chunk id或旧候选不可代替它。AG085产品scanner与含AG071的实际父树逐字节相等，其两主控探针在head与父均通过；真正红灯来自两个正常编译后隔离语义变异，不能写成父树缺陷复现。

索引从69条增至72条，只新增这三个已合并子范围；所有旧记录的原始JSON对象、顺序和各自版本绑定保持原字节。顶层仍绑定唯一v4.2真源和v4.1→v4.2谱系。命令中的offline是检查器要求的定向入口文法，原始真实执行为cargom --locked镜像全量，没有新执行offline。私有输入清单绑定实际Git父/树/blob、唯一主日志、真实计数及全部验收/探针摘要；原QA、方案正文、合同和归档仍只在本地。

以下E状态按默认完整范围计算；记录条数只表示已验子范围的证据数量，不能换算成完成百分比。缺真实provider/宿主/付费材料的状态继续单列，不阻止无依赖的本地工程推进。

| E任务 | 默认完整范围状态 | 已验子范围记录数 |
|---|---|---|
| E00 | implemented_not_verified | 1 |
| E01 | blocked | 2 |
| E02 | in_progress | 4 |
| E03 | in_progress | 2 |
| E04 | in_progress | 2 |
| E05 | blocked | 3 |
| E06 | blocked | 1 |
| E07 | in_progress | 5 |
| E08 | in_progress | 11 |
| E09 | in_progress | 10 |
| E10 | in_progress | 4 |
| E11 | in_progress | 1 |
| E12 | in_progress | 3 |
| E13 | in_progress | 5 |
| E14 | in_progress | 3 |
| E15 | planned | 0 |
| E16.1 | in_progress | 1 |
| E16.2 | in_progress | 3 |
| E16.3 | in_progress | 1 |
| E16.4 | blocked | 2 |
| E16.5 | in_progress | 5 |
| E16.6 | in_progress | 1 |
| E16 | in_progress | 0 |
| E17 | in_progress | 1 |
| E18 | in_progress | 1 |

AG069、070、075、076、077、078、079的本地实现继续保留，须叠放到AG086下一安全主线后按实际新epoch重新跑八门与独立案例；旧9b或更早的通过只属于历史。AG080/081按其依赖与裁决推进，AG084仍只有prepared卡，未实施未验收。完整三reader→持久候选→开发选择→真实新使用→撤销工程链仍未闭合；不得拿真实provider或正式批准缺证停止全部本地开发，也不能把未验证开发材料变成可信应用证据。

独立阻塞仍保留：AG082公共外发缺具体授权；AG072原远端删除被自动审批拒绝，未重试；AG056原公共推送目的地/敏感输出授权阻塞仍未被workflow scope变化解除。真实Claude、合格隔离执行、付费/模型凭据、E14机制合同、E15实验、部分GC/终态合同及历史T001–T126等价证明继续逐范围登记。071/072既往公共推送阻塞已由后续实际授权与真实合并解决，不能把旧事件当成当前未合并事实。维度仍为subset_only、not_full_route_complete、legacy_equivalence_unverified、effect=not_claimed，E17/E18缺省关闭，无真实模型、效果、生产或发行声明。

上次台账PR115于2026-10-02T14:01:05Z合并，其六小时更新截止20:01:05Z已经超过。本批在AG085真实合并及清理后冻结至2026-10-03T12:12:42.445255+00:00事件；此前超期及过程错误照实保留。旧AG072折入源结束于当前完整草稿字节223498；其后一字节LF为事件分隔。新增区间[223499,561069)原文337570B、SHA-256 4e3b8e7ccb213fc8283b8b888e74e83a24dea802486f619d60c23ccff26b6d28，转换337980B、SHA-256 fdd3798f057d31b784e994382b0653f9c6e8a5fc46f576f3f596bc4a4578814d。205个行首标题仅增加两个#，非标题原文与旧公开历史全部保留；后续事件留待下一批。

### 第四轮44项逐任务历史草案（AG111准备；完整逐字段守恒未闭）
这里是将来公开版候选，不是已实施台账：旧首次与后来Claude二验分列；44唯一PR98–141，不造第45。原plan/日期/费用/拒绝/失败与未知保留，不新增v4.3 verified、T等价、真实模型/宿主/效果/发行信用。
逐字段候选及原全文由本地out/audit-20260930/qa/codex-r7-ag111保存；这些证据不上传。以下任务段取原已结构化first胶囊作有时点的语义草案，未将完整事件journal或JSON状态转储公开复制。
### 历史完整原像导航（本地只读；不授当前验收）
原事实保全与公开摘要是两项义务：以下完整原像及精确字节跨度保留原事实；每任务范围、失败、计数、返修、权限及不可声明仍须准确。本地原像不自动证明所有公开摘要语义通过。
- 原像锚 `historical-original-ledger`：`out/audit-20260930/qa/codex-r7-ag111/M12-5958b210-original-ledger-two-layer-r1.md`；SHA256 `bad32cb9a10acc82232f6d1621efe7e1d96dd8a83d375fe25dce1c5e8ea6376e`；2186215 bytes/8920物理行。
- 原像锚 `historical-original-codex_first`：`out/audit-20260930/codex-r4-second-acceptance.md`；SHA256 `0485f0db9ce38df5539a646efcdd6b081cb6fa8c37062691cc4ed76c880a61f0`；231564 bytes/998物理行。
- 原像锚 `historical-original-claude_second`：`out/audit-20260930/claude-r4-second-acceptance-20261011.md`；SHA256 `4482e1c1860435a4ee4ade1efa17ce75cc5c5d2c73cfd00255eaf434fc016bc9`；52386 bytes/322物理行。
- 原像锚 `historical-original-controller`：`out/audit-20260930/qa/codex-r7-ag111/historical-controller-J46-J62-readonly-slice.md`；SHA256 `62644478a814d131388a301dce71be85b298d0d5174f2704bdf8d6dffef6803e`；72483 bytes/238物理行；原控制器L5249–5486对应归档物理L1–238。
- 全5745原事实ID及原LF SHA/字节跨度：`out/audit-20260930/qa/codex-r7-ag111/M12-full5745-original-byte-span-navigation-prepared-r1.json`；SHA256 `72853c1c6781411e029912e3c37991c6eb318e0fa3624191cb90f0d90ab34338`。3698共享成员仍为共享，不强塞给单PR；没有缺失ID，也没有新增verified信用。
有限未定位原段暂保解释：完整本地原像不替代尚未复核的必要公开原文字；本稿保留PR116原胶囊、三未知史原声明及必要原段，注明not_located。这一有限呈现解释标“待二次验收裁决”；旧5591待审标签原样保留，并非新增规范阈值。

### PR #98 — AG-060（第四轮历史）
冻结M6角色：公开head 1112b7782a3b951f2bee24105b3a58b5b5855a3a；本地验收head 1112b7782a3b951f2bee24105b3a58b5b5855a3a；实际merge 37fa6f08e9ef779b796b9713bc4664511321aa7e；mergedAt 2026-10-01T11:24:50Z。
合并父 804302602efe575d92973be20c532d264019f9aa, 1112b7782a3b951f2bee24105b3a58b5b5855a3a；tree cafb1a34b9edb3b9f06fb64c9cd2f18892796ac8。原双父合并角色；不是本轮网络重查或重新执行。
**历史first阶段（Codex原记录L11–19；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-060 / PR #98 — 第三轮台账与索引
历史first事实：- E00/E16.6；V001/V071/V072/V073/V074/V080/V098仅证据登记/基线/迁移/消费者复核子范围。
历史first事实：- 卡：cards/AG-060.md；SHA256 ffd0a0f06b75e3325e68394bbbad3daa7eebe909fb4b39c95d6f196cc9a4fd6c。无返修裁决。
历史first事实：- 基线804302602efe575d92973be20c532d264019f9aa → 验收head 1112b7782a3b951f2bee24105b3a58b5b5855a3a，无需叠放；merged_sha 37fa6f08e9ef779b796b9713bc4664511321aa7e，mergedAt 2026-10-01T11:24:50Z。两树均cafb1a34b9edb3b9f06fb64c9cd2f18892796ac8。
历史first事实：- 全量日志qa/ctrl-ag060-1112b77.log*；fmt/clippy/workspace/build/两组smoke/checker/unittest退出码全部0。workspace1124通过0失败，86结果行；unittest48；support_scope定向14/0（qa/ctrl-ag060-support-scope-1112b77.log）。
历史first事实：- 反例：qa/probe-ag060/probe.py（SHA256 7075219643b9ad755d6f1dddfb48ba9a49c96d05238d7ba85efe44e4711af588）；登记探针qa/probe-ag060/registration.py。head日志qa/ctrl-ag060-adversarial-head-1112b77.log全通过；父树日志qa/ctrl-ag060-adversarial-base-8043026.log因缺四条登记失败，四新记录的源码树/祖先/blob/输入与日志绑定均验证。T1旧输入摘要、T2复用日志、T3错误merged identity、T4新输入摘要篡改全部拒绝；T1/T2基线也拒绝，是边界核验。
历史first事实：- 不可声明产品新能力/整体E-V/真实效果/可发行/历史包等价。旧历史输入部分缺失；新四份本地清单完整。
历史first事实：- 返修0；admin=false；冲突无。偏离/适配：按第5节索引任务采用Python篡改反例而非Rust append.rs；工具未提供主控实际模型元数据，署名Codex未虚构型号。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E00/E16.6；未入当前98单目标索引：台账/索引PR。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-060.md`，文件SHA256 `ffd0a0f06b75e3325e68394bbbad3daa7eebe909fb4b39c95d6f196cc9a4fd6c`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag060-1112b77.log.test`，文件SHA256 `dc1d3bdbf0abd18b69284655b17f5e0ed6fe0842c2f09358dc675e57b80a48dd`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag060/probe.py`，文件SHA256 `7075219643b9ad755d6f1dddfb48ba9a49c96d05238d7ba85efe44e4711af588`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag060/registration.py`，文件SHA256 `a4b24b363696e30c3b09b6fdc82a8a3b7ff661c1b0e73af70b8745afc9ef8434`。
### PR #99 — AG-053（第四轮历史）
冻结M6角色：公开head a2c62326c7ca87a724da16ee4cfd133462b49dbe；本地验收head a2c62326c7ca87a724da16ee4cfd133462b49dbe；实际merge 3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360；mergedAt 2026-10-01T11:51:03Z。
合并父 37fa6f08e9ef779b796b9713bc4664511321aa7e, a2c62326c7ca87a724da16ee4cfd133462b49dbe；tree b5855ddf193f72b1b9dd43622e01d54b8c887c4d。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E08/verified_subscopes/6`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E08.ag053_bounded_dependency_snapshots；description = Bounded dependency snapshot recursion
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = a2c62326c7ca87a724da16ee4cfd133462b49dbe；pr = https://github.com/acosmi/RSIAgent/pull/99；merged_sha = 3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360；pr_state = merged
历史原测试登记：test_entry = crates/evo-storage/tests/local_export.rs；command = cargo test --locked --offline -p evo-storage --test local_export；exit_code = 0
历史原证据登记：input_digest = 2ece3ccd3dcedafc1d5d53301ef73e27db6888ea35fa547260be9e47f1839c00；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-053-a2c6232.json；log_path = out/audit-20260930/qa/ctrl-ag053-a2c6232.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E08/E16.5; sections 3.3.1 L319, 11 L1081, E08 L1295/L1299, E16.5 L1451/L1453; V017/V069/V075 export dependency snapshots and V038 bounded refusal: preserve the 10000-node bound, root/all-kind snapshot semantics, namespace, order, errors and fingerprints while limiting recursion and using keyed closure joins
历史原实际结果（原时点）：actual_result = controller independent full re-run at a2c6232: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; local_export 11 passed / 0 failed; separate evo-storage lib 21 passed / 0 failed including 6 new dependency snapshot/VM tests (qa/ctrl-ag053-storage-lib-a2c6232.log); workspace 86 result lines, 1130 passed / 0 failed; checker unittest 48 passed; AG-061-R1 uses consumer entry because root lib tests are outside the index grammar; original storage command and test blob retained in input manifest
历史原风险与不可声明：risk = No absolute bound on total edge scanning or queue memory, no N+1 removal, complete cleanup, production performance, Linux, benefit or release claim
历史原回滚约束：rollback = revert PR #99 through an independent revert PR; accepted-head parent 37fa6f08e9ef779b796b9713bc4664511321aa7e; preserve immutable evidence, revocation, budget, cost and audit facts
**历史first阶段（Codex原记录L28–36；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-053 / PR #99 — 有界依赖快照递归
历史first事实：- E08/E16.5；V017/V069/V075仅导出依赖核验，V038仅受限规模拒绝。卡cards/AG-053.md SHA2562057df98e3eb45dd074456ae0858e68cec8a22efab1e12eef63322f74ef6c59c；裁决cards/AG-053-R1.md SHA2562813cdaee00e8e4da7cb3e432d42f03de2bfe7b29fe861f3c31014b5b869c2ef。
历史first事实：- 基线804302602efe575d92973be20c532d264019f9aa → 交付f5214bcae56fd5e226308ad03f877dfb35195ec4，叠放到37fa6f08e9ef779b796b9713bc4664511321aa7e → 验收a2c62326c7ca87a724da16ee4cfd133462b49dbe，单提交补丁逐字节一致（qa/ctrl-ag053-before-rebase.patch及after-rebase.patch）。merged_sha 3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360，mergedAt2026-10-01T11:51:03Z；树双方b5855ddf193f72b1b9dd43622e01d54b8c887c4d。
历史first事实：- 全新target-ctrl-ag053，qa/ctrl-ag053-a2c6232.log*全部8项退出0；workspace1130/0（main1124+6），81二进制+5doc组，unittest48。定向qa/ctrl-ag053-storage-lib-a2c6232.log 21/0，主控VM390103→390104、1099→1084，与实施结果同。
历史first事实：- 反例qa/probe-ag053/append.rs SHA256ba5039042d17e5fb8fa252fc43d20194bd81f8c41c1a550616cac15ac009f6b2；源source-local_export-37fa6f0.rs SHA25662e8afcacdd8146e79f24f7eba3ea917d9c68ecb298f1a0a769483df6e464f7c；head日志qa/ctrl-ag053-adversarial-head-a2c6232.log，父日志qa/ctrl-ag053-adversarial-base-37fa6f0.log，各2/0且清理后干净。P1深层共享环依赖正文变更拒绝发布无完成残留；P2跨namespace匹配ID无法重绑快照。均为基线同过的语义保持边界。
历史first事实：- 资源缺陷基线4/2失败（qa/impl-ag053/baseline-r1-tests.log）；长链747060→2787061、无关边4075→12804005，head6/0并全量复验。R1夹具ANALYZE只隔离长链外层LIMIT；原中断130保留，不能声称完成。
历史first事实：- verified：§3.3.1 L319/§11 L1081/E08 L1295,L1299/E16.5 L1451,L1453既有10000快照界、递归提前拒绝和查询按节点键访问，保留返回/错误/排序/指纹/namespace。不可声明总边扫描/内存绝对有界、N+1消除、完整清理、生产容量/时延、Linux、真实效果/可发行。
历史first事实：- 返修0；R1测试机制裁决1；admin=false；无冲突。偏离：验收正文写入EOF后ready/merge先完成，正文随即补写并读回确认；全部验收与本地记录在合并前已完成，未合并未验收树。已调整后续显式前置检查。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E08/E16.5；E08.ag053_bounded_dependency_snapshots [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-053.md`，文件SHA256 `2057df98e3eb45dd074456ae0858e68cec8a22efab1e12eef63322f74ef6c59c`。
- 原卡导航：`out/audit-20260930/cards/AG-053-R1.md`，文件SHA256 `2813cdaee00e8e4da7cb3e432d42f03de2bfe7b29fe861f3c31014b5b869c2ef`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag053-a2c6232.log.test`，文件SHA256 `a4bcb6482c24c1a06f4c258318c6830883289f1eac9268bbc295cdbd9438a6ce`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag053/append.rs`，文件SHA256 `ba5039042d17e5fb8fa252fc43d20194bd81f8c41c1a550616cac15ac009f6b2`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag053-adversarial-head-a2c6232.log`，文件SHA256 `f6c2eecb7f9834de342709982e013fa3eba1a077ce3755d16673071dc2e71d71`。
### PR #100 — AG-052（第四轮历史）
冻结M6角色：公开head 652d7cac0d6cf1fa086e96b6ac9554ed6326b1da；本地验收head 652d7cac0d6cf1fa086e96b6ac9554ed6326b1da；实际merge 43f0022478d58dd8a8e6492cffb3aa24463b3fce；mergedAt 2026-10-01T12:08:10Z。
合并父 3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360, 652d7cac0d6cf1fa086e96b6ac9554ed6326b1da；tree 430461d69007524b1f531ed3ec457f722f86dcff。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E05/verified_subscopes/1`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E05.ag052_versioned_batch_verdicts；description = Versioned complete-batch verdict preservation
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 652d7cac0d6cf1fa086e96b6ac9554ed6326b1da；pr = https://github.com/acosmi/RSIAgent/pull/100；merged_sha = 43f0022478d58dd8a8e6492cffb3aa24463b3fce；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/streaming_verdicts_v42.rs；command = cargo test --locked --offline -p evo-engine --test streaming_verdicts_v42；exit_code = 0
历史原证据登记：input_digest = 553355fd10dd745a44e29bb94af5f19f096fc79a102d655ad7fe607bfbd9da44；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-052-652d7ca.json；log_path = out/audit-20260930/qa/ctrl-ag052-652d7ca.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E05 L1253/L1257/L1259, sections 3.4 L337, 3.4.1 L372 and 6.6 L709/L711; V010/V012/V084/V085 program scope: explicitly selected control v2 preserves eligible statistical verdicts with conservative negative-LCB/cost-saving postprocessing; v1 frozen bytes, digest and reasons remain; full registration binding is checked before calculation and final persistence; promotion gates remain closed
历史原实际结果（原时点）：actual_result = controller independent full re-run at 652d7ca: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; streaming_verdicts_v42 10 passed / 0 failed; workspace 87 result lines, 1142 passed / 0 failed; checker unittest 48 passed; test exit 0
历史原风险与不可声明：risk = No new UCB algorithm, full cost proof, real-provider samples, promotion authority, universal v2 upgrade, Linux or effect claim; complete-batch ProgramFixture evidence stays non-promotable
历史原回滚约束：rollback = revert PR #100 through an independent revert PR; accepted-head parent 3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360; preserve immutable evidence, revocation, budget, cost and audit facts
**历史first阶段（Codex原记录L44–53；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-052 / PR #100 — 版本化完整批次判定
历史first事实：- E05；V010/V012/V084/V085仅冻结控制/完整程序批次判定/恢复/晋级拒绝。卡cards/AG-052.md SHA2563ec6598f40da645833d3162e818f3f49af3b41a8a867f6478573c7317006de24；派发前裁决cards/AG-052-R1.md SHA256c02b2f32fe1661c350da08bb4193852d4939c076128f076182faab8a0edf6275。
历史first事实：- 基线37fa6f08e9ef779b796b9713bc4664511321aa7e→交付fc381036482727454f7b008a51fec99a8e2d581b；rebase到3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360→验收652d7cac0d6cf1fa086e96b6ac9554ed6326b1da。单提交patch逐字节一致（qa/ctrl-ag052-before-rebase.patch、after）。merged_sha43f0022478d58dd8a8e6492cffb3aa24463b3fce，mergedAt2026-10-01T12:08:10Z，两树430461d69007524b1f531ed3ec457f722f86dcff；实际merge父为已验base/head。
历史first事实：- qa/ctrl-ag052-652d7ca.log*全部8项退出0，workspace1142/0=1130+12；82二进制+5doc组，unittest48。原实施logs/report归qa/impl-ag052，基线3/7失败，新10集成+2单测过，沙箱HTTP失败已完整保留。
历史first事实：- 主控probe-ag052/append.rs SHA2560a370309dccf9c761791339262ef51e5fd541531dafcbb268ca007b9c2fe20cf；固定源source-streaming-v1-37fa6f0.rs SHA25636d3bdb5b77946ab6715624eaefc6cf5b0f2a45ed67c65a6e9fafda9e32cc37b，只有原SCHEMA替换v1字面量以两树同测旧冻结控制。head日志qa/ctrl-ag052-adversarial-head-652d7ca.log 2/0；父qa/ctrl-ag052-adversarial-base-3aa3804.log 0/2，旧代码两例都错误生成CompleteBatch。P1合法非schema审批主体改动；P2ticket登记摘要被替换；新headConflict且重复无artifact/audit写入。两树clean。
历史first事实：- verified：E05L1253/L1257/L1259、§3.4L337、§3.4.1L372、§6.6L709/L711；显式v2保留统计结论、未有UCB确认的negative_lcb退化不声明确认、节省无统计证明仍Inconclusive；v1完整typed字节/digest/reasons与恢复保持、release gate仍拒绝。源core算法/阈值不变。
历史first事实：- 不可声明真实效果/成本统计/新固定样本UCB/生产或晋级权/全部请求自动v2/整体E-V/Linux。第二提交闸代码复核但未穷举并发调度。回滚保留历史/费用，旧程序不支持v2必须失败关闭。
历史first事实：- 返修0；派发前R1裁决1；admin=false；冲突无。发布适配：辅助chat审批拒绝外发，root核本chat直接用户指令及精确payload/remote后获批推送建PR，拒绝原文留report。
历史first事实：- 过程偏离：验收正文已在merge前成功读回；但合并前fetch TLS失败未中止后续merge工具调用。合并后验证实际父未前进、树完全一致。后续已决定所有合并使用单一fail-fast脚本，以成功刷新/正文/readback/head/base/clean为硬前置，不再依多工具调用顺序假定成功。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E05；E05.ag052_versioned_batch_verdicts [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-052.md`，文件SHA256 `3ec6598f40da645833d3162e818f3f49af3b41a8a867f6478573c7317006de24`。
- 原卡导航：`out/audit-20260930/cards/AG-052-R1.md`，文件SHA256 `c02b2f32fe1661c350da08bb4193852d4939c076128f076182faab8a0edf6275`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag052-652d7ca.log.test`，文件SHA256 `3c7a72f6ea67f293cd442c0e291947fe172b7cc1d73e9e9f4a852905f6636e9b`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag052-adversarial-head-652d7ca.log`，文件SHA256 `fd6fe49859098d504427d51b797e7685106f25eff57baab14d6854550fb7fc62`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag052-adversarial-base-3aa3804.log`，文件SHA256 `6eb0cd273df3458ecd8c9488c2e2193432436877a363cbcbb56c31ab8e9154e6`。
### PR #101 — AG-059（第四轮历史）
冻结M6角色：公开head 8257e6a73304c250b545ead3c0bc9409cf2daa35；本地验收head 8257e6a73304c250b545ead3c0bc9409cf2daa35；实际merge 296b5a07a7bf911d02454b683d5732e9d38e3ae5；mergedAt 2026-10-01T12:31:01Z。
合并父 43f0022478d58dd8a8e6492cffb3aa24463b3fce, 8257e6a73304c250b545ead3c0bc9409cf2daa35；tree 87dc2da1cb72494b7241792c96b4eb2341009a1f。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（历史已撤出对象（不计当前98；原错误/旧复核标签仍保留））：`out/audit-20260930/qa/codex-r7-ag124/withdrawn-records.json`，JSON pointer `/records/3/record`，文件SHA256 `d71db404637255b0600c8dcdeae8ac3725cad9360e3cb65e51f558da368c19fd`。
历史原登记身份：id = E08.ag059_manifest_prewrite_gate；description = Manifest dependencies checked before package writes
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = not_affected
历史原提交角色：source_sha = 8257e6a73304c250b545ead3c0bc9409cf2daa35；pr = https://github.com/acosmi/RSIAgent/pull/101；merged_sha = 296b5a07a7bf911d02454b683d5732e9d38e3ae5；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/write_side_revocation_gate_v42.rs；command = cargo test --locked --offline -p evo-engine --test write_side_revocation_gate_v42；exit_code = 0
历史原证据登记：input_digest = abf5d40aba4546aff5490af3098279bb0f37688ba55719e2125d207a4fe3608d；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-059-8257e6a.json；log_path = out/audit-20260930/qa/ctrl-ag059-8257e6a.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E08/E16.2; sections 11.3 L1110 and 11.5 L1132, E08 L1295/L1299; V017/V075: resolved manifest and request source refs are checked jointly before first package write; revoked inputs leave no Prepared envelope, audit, edge or blob; V038 existing joint closure bound; missing manifest refs retain quarantine semantics
历史原实际结果（原时点）：actual_result = controller independent full re-run at 8257e6a: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; write_side_revocation_gate_v42 21 passed / 0 failed; workspace 87 result lines, 1150 passed / 0 failed; checker unittest 48 passed; test exit 0
历史原风险与不可声明：risk = Storage publication still has the documented id-only other-kind tombstone late-refusal residue; no physical erasure, unknown-schema cleanup, real host support, production performance, Linux, effect or full E/V claim
历史原回滚约束：rollback = revert PR #101 through an independent revert PR; accepted-head parent 43f0022478d58dd8a8e6492cffb3aa24463b3fce; preserve immutable evidence, revocation, budget, cost and audit facts
**历史first阶段（Codex原记录L61–68；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-059 / PR #101 — manifest内容依赖首写前联合闸
历史first事实：- E08/E16.2；V017/V075已解析manifest来源撤销阻断，V038既有联合闭包界。卡cards/AG-059.md SHA25642340897f0469180c54bd2c6819f285c795ab6ffbd4112aea2fc152837ceb9a8；R1 cards/AG-059-R1.md SHA25623ddc7a475437ad2e36d5dfbfc63942bd22281d8170968738a8d0d528ec92c71。
历史first事实：- 基线37fa6f08e9ef779b796b9713bc4664511321aa7e→交付4c403ebc68c8c66bfd2c45f74778f4cef884f189；rebase到43f0022478d58dd8a8e6492cffb3aa24463b3fce→验收8257e6a73304c250b545ead3c0bc9409cf2daa35，一提交patch逐字节相同。merged_sha296b5a07a7bf911d02454b683d5732e9d38e3ae5，mergedAt2026-10-01T12:31:01Z；双方tree=87dc2da1cb72494b7241792c96b4eb2341009a1f，merge实际父为已验base/head。
历史first事实：- qa/ctrl-ag059-8257e6a.log*：全新target，8项exit0，1150/0=1142+8，82二进制+5doc组，Python unittest48；实施原始含环境失败、夹具失败全部在qa/impl-ag059，最终report handoff.md。
历史first事实：- probe-ag059/source-write-side-3aa3804.rs SHA256e2f9b6ff8235ba5da2fe4edb643cd19f526a4d4501bb2336a08055dd5feced4d，append.rs SHA2561bf7fb8c4c441a7dde7046b1b383c7626c8f1d1145e9ebc4294645257b2683b8。head日志ctrl-ag059-adversarial-head-8257e6a.log 2/0 exit0；父ctrl-ag059-adversarial-base-43f0022.log 0/2 exit101，P1缺失与脱敏已解析依赖混合（两序/重复），P2同名不同kind资产的撤销improver不能被live skill掩盖；正向stage/quarantine保持。基线每例留下Prepared/边/audit/blob，新head无变更；两树clean。
历史first事实：- verified §11.3L1110/§11.5L1132/E08L1295,L1299，联合来源在首写前拒，保持原request错误优先/未知依赖quarantine/冻结摘要/后续门。不可声明storage-kind精确/物理擦除/未知schema清理/真实宿主或导入导出/吞吐时延/Linux/全部E-V/收益或发行。回滚参考43f0022478d58dd8a8e6492cffb3aa24463b3fce，保留撤销/账单事实。
历史first事实：- 产品返修0；R1注释授权1；admin=false；无冲突。GraphQL创建EOF后经裁决等价REST创建；初次只读AGENTS文件名搜索范围偏离已披露。实施最后head断言失败是主控接管rebase，原失败保留。主控预rebase fetch失败脚本立即停，重试后继续；本次merge全程fail-fast且前置全部通过。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E08/E16.2；E08.ag059_manifest_prewrite_gate [withdrawn，旧通过不计当前，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-059.md`，文件SHA256 `42340897f0469180c54bd2c6819f285c795ab6ffbd4112aea2fc152837ceb9a8`。
- 原卡导航：`out/audit-20260930/cards/AG-059-R1.md`，文件SHA256 `23ddc7a475437ad2e36d5dfbfc63942bd22281d8170968738a8d0d528ec92c71`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag059-8257e6a.log.test`，文件SHA256 `95c6d279e28b4efe475e27e1ae72c898748aa972fb74abcd4e0459806a7f5978`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag059/source-write-side-3aa3804.rs`，文件SHA256 `e2f9b6ff8235ba5da2fe4edb643cd19f526a4d4501bb2336a08055dd5feced4d`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag059-adversarial-head-8257e6a.log`，文件SHA256 `54cba98f93081618e86afa717389e07c528b4ec9cce631c6d405f214397bdfd4`。
### PR #102 — AG-061（第四轮历史）
冻结M6角色：公开head 46f920b979e151c022908b4d10cc8f97cfb52a5e；本地验收head 46f920b979e151c022908b4d10cc8f97cfb52a5e；实际merge 35b4d4e08df121e68467b0783b29303e9fa31d94；mergedAt 2026-10-01T12:52:57Z。
合并父 296b5a07a7bf911d02454b683d5732e9d38e3ae5, 46f920b979e151c022908b4d10cc8f97cfb52a5e；tree 46f18b66079abdb80244480eb52ecd815af9642d。原双父合并角色；不是本轮网络重查或重新执行。
**历史first阶段（Codex原记录L75–82；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-061 / PR #102 — 第四轮首批台账与索引
历史first事实：- E00/E16.6，V001/V071/V072/V073/V074/V080/V098限三项证据登记/追溯。卡AG-061.md SHA256beb3c160ec5d99105aadec68bb4f567f8edfe3b2c32ae368975dea4d496eb9ab；R1 SHA256d5b606eaf98671dc939cdceda8ae4a54c716730c8ba475374d8fe9f186dd96a7。
历史first事实：- 基线296b5a07a7bf911d02454b683d5732e9d38e3ae5→head46f920b979e151c022908b4d10cc8f97cfb52a5e，无叠放；merged_sha35b4d4e08df121e68467b0783b29303e9fa31d94，mergedAt2026-10-01T12:52:57Z；tree46f18b66079abdb80244480eb52ecd815af9642d双方相等，实际父已核。
历史first事实：- qa/ctrl-ag061-46f920b.log*全部8项exit0，1150/0、82二进制+5doc组，Python48；ctrl-ag061-support-scope-46f920b.log14/0。
历史first事实：- probe-ag061/probe.py SHA25650df929b67d95c11a22e33965bdc603122d185800a7cae58613349b81c3bf101；head ctrl-ag061-adversarial-head-46f920b.log所有git/tree/祖先/blob/日志摘要/计数和5篡改、正向、clean通过。父ctrl-ag061-adversarial-base-296b5a0.log缺3新增登记退出1，旧输入篡改/正向保持。T0旧input、T1AG053新input、T2跨scope日志、T3错误merge、T4AG059input拒绝；AG053额外storage21/0及6快照单测绑定已核。
历史first事实：- verified仅L1181–1189/L1459–1463/L2162–2166追溯。不可声明新产品/全部E-V/历史归并/真实效果/发行；旧历史input缺失保持。回滚参考基线并保留证据。
历史first事实：- 产品返修0、主控索引R1适配一次；admin=false，无冲突。Python替代Rust按启动§5；初次probe没复制input而3篡改未拒，原*-setup-failed.log保留，同夹具复制并finally清理后两树重验成功。正文首调用TLS失败重试通过，未跨失败继续合并。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E00/E16.6；未入当前98单目标索引：台账/索引PR。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-061.md`，文件SHA256 `beb3c160ec5d99105aadec68bb4f567f8edfe3b2c32ae368975dea4d496eb9ab`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag061-46f920b.log.test`，文件SHA256 `d7a9ffb90cc04b41f5c3dd814c462baeeec5b10fd1fee811e334c4cd8990f822`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag061/probe.py`，文件SHA256 `50df929b67d95c11a22e33965bdc603122d185800a7cae58613349b81c3bf101`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag061-adversarial-head-46f920b.log`，文件SHA256 `4780fd5b8cada6a8b74814b8fe12dbd975d4fd78f9d31c583a057fdf98095a30`。
### PR #103 — AG-055（第四轮历史）
冻结M6角色：公开head c0ea35de8b028f849f89008f7edca2852e412812；本地验收head c0ea35de8b028f849f89008f7edca2852e412812；实际merge ba01a2e8489ceef6d1350f0babdac46d816071ec；mergedAt 2026-10-01T13:03:52Z。
合并父 35b4d4e08df121e68467b0783b29303e9fa31d94, c0ea35de8b028f849f89008f7edca2852e412812；tree bfc3cbfc6c4734a6771526ef2828eeca8e3e1628。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E02/verified_subscopes/1`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E02.ag055_v1_wire_goldens；description = v1 wire and tool descriptor regression goldens
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = c0ea35de8b028f849f89008f7edca2852e412812；pr = https://github.com/acosmi/RSIAgent/pull/103；merged_sha = ba01a2e8489ceef6d1350f0babdac46d816071ec；pr_state = merged
历史原测试登记：test_entry = crates/evo-core/tests/v1_golden.rs；command = cargo test --locked --offline -p evo-core --test v1_golden；exit_code = 0
历史原证据登记：input_digest = 6d63c971e05bbe6eae18491af7d4e423a1dc8c048adc7762bebf5a98bf9656ce；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-055-c0ea35d.json；log_path = out/audit-20260930/qa/ctrl-ag055-c0ea35d.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E02; sections 5.1 L453/L455 and 6.1 L605/L609, E02 L1213/L1215, V002 L1502 and reproducible fixtures L2166: frozen typed bytes for five v1 requests, Strategy/legacy Evaluation and four real tool descriptors within 2000 bytes; independent descriptor perturbations detected
历史原实际结果（原时点）：actual_result = controller independent full re-run at c0ea35d: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; v1_golden 6 passed / 0 failed; workspace 89 result lines, 1158 passed / 0 failed; checker unittest 48 passed; two controller probes pass on head and actual parent; core golden 6 plus MCP golden 2 in the full suite; two extra real descriptor mutations fail and restored controls pass
历史原风险与不可声明：risk = Tests and synthetic fixtures only; no RFC8785, new protocol, real host certification, full E02/V002, product effect or release claim
历史原回滚约束：rollback = revert PR #103 through an independent revert PR; accepted-head parent 35b4d4e08df121e68467b0783b29303e9fa31d94; retain immutable evidence, frozen plans, costs and revocation state
**历史first阶段（Codex原记录L87–95；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-055 / PR #103 — v1请求与工具描述符golden
历史first事实：- E02/V002仅v1回归证据；卡cards/AG-055.md SHA256853308a8c4f301df3cde2cc8e3a3c3382d9ba2ae6ac0901ef2dfe1f89dac7060，无返修裁决。
历史first事实：- 原基线43f0022478d58dd8a8e6492cffb3aa24463b3fce→交付783a3132900418337b6f647f16605d773d1a0d0d；叠到35b4d4e08df121e68467b0783b29303e9fa31d94→验收c0ea35de8b028f849f89008f7edca2852e412812，单提交patch逐字节同（qa/ctrl-ag055-before/after-rebase.patch）。merged_shaba01a2e8489ceef6d1350f0babdac46d816071ec，mergedAt2026-10-01T13:03:52Z；两树bfc3cbfc6c4734a6771526ef2828eeca8e3e1628，实际merge父核对。
历史first事实：- 全新target-ctrl-ag055，qa/ctrl-ag055-c0ea35d.log*8项exit0，1158/0=1150+8，84二进制+5doc组，Python48。原实施qa/impl-ag055含基线缺fixture运行失败、全部初次错误及最终成功；source路径以git为准（report有两个重复crates字样笔误）。
历史first事实：- probe-ag055/source-core-golden-c0ea35d.rs SHA256b24ae22a8dfcf2c8aa8ee7b60ea525d8f00fec99516b04c85ca3032a59b7780a；append.rs SHA256983d082b2cfb7e81ab4a1d45cf09409c7414c0a6bed9ba19bf6008af58bc45ac。ctrl-ag055-adversarial-head-c0ea35d.log及base-35b4d4e.log两树2/0 exit0、clean；P1原JSON转义身份/重复字段5类拒绝，P2旧Evaluation扩展/省略meta与null/signedzero字节指纹。是保持边界，非旧产品缺陷。
历史first事实：- root额外mutations.py SHA2564d352041005e0024735d546c669fcbd22d14e9d513e68f72a2e9101ba7a1b296，inspect添加management枚举454B、feedback默认分类变更722B，两个实际descriptor仍≤2000但真实golden测试各exit101/1失败；每次恢复后exit0/1通过，clean。日志ctrl-ag055-mutation-*、restored-*、mutations-summary.log。
历史first事实：- verified §5.1L453,L455/§6.1L605,L609/E02L1213,L1215/V002L1502/L2166的typed字节/五请求/四实际descriptor/Strategy与旧Evaluation回归。不宣称RFC8785/新产品或协议/HTTP Application真实兼容/宿主认证/全E-V/效果/发行。回滚只移除新增测试/fixture，参考35b4d4e并保留证据。
历史first事实：- 返修0，admin=false，无冲突。子代理发布auto-review拒绝由root重新核人类授权/精确payload/远端后获准，拒绝原文保留；没有绕过。merge前fetch短暂TLS重试成功，硬前置完整。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E02；E02.ag055_v1_wire_goldens [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-055.md`，文件SHA256 `853308a8c4f301df3cde2cc8e3a3c3382d9ba2ae6ac0901ef2dfe1f89dac7060`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag055-c0ea35d.log.test`，文件SHA256 `198e9efe05e2df57bb030a49eb7513f1aa1b47fe1ff51c800c83044375dcbd01`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag055/source-core-golden-c0ea35d.rs`，文件SHA256 `b24ae22a8dfcf2c8aa8ee7b60ea525d8f00fec99516b04c85ca3032a59b7780a`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag055-adversarial-head-c0ea35d.log`，文件SHA256 `4fac3619268797e946aed55329ece782356fee40811646b0b8f73a9c26042109`。
### PR #104 — AG-054（第四轮历史）
冻结M6角色：公开head f6aa729a467f10f8b15b2d7b20e9a9d1c7d80351；本地验收head f6aa729a467f10f8b15b2d7b20e9a9d1c7d80351；实际merge 9b08c4b2fd882abab65f48ea0af8f23b13a3b50d；mergedAt 2026-10-01T13:34:48Z。
合并父 ba01a2e8489ceef6d1350f0babdac46d816071ec, f6aa729a467f10f8b15b2d7b20e9a9d1c7d80351；tree 437240ad205e1009d2dc51681e20d39b9ef93a6c。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（历史已撤出对象（不计当前98；原错误/旧复核标签仍保留））：`out/audit-20260930/qa/codex-r7-ag128/ag124-ag054-clock-recheck-correction.json`，JSON pointer `/record_to_withdraw`，文件SHA256 `6c31386789cf9471af8e9ba916bdf091be7cac5340b1fd36e260bc9230e5a458`。
历史原登记身份：id = E04.ag054_typed_predispatch；description = Typed pre-dispatch refusals and safe reserved-call cleanup
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = f6aa729a467f10f8b15b2d7b20e9a9d1c7d80351；pr = https://github.com/acosmi/RSIAgent/pull/104；merged_sha = 9b08c4b2fd882abab65f48ea0af8f23b13a3b50d；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/broker.rs；command = cargo test --locked --offline -p evo-engine --test broker；exit_code = 0
历史原证据登记：input_digest = 6341f8fad1ced29375cd8c3a6cb6db77c4ec4ed96be32165d73ccadbf884cd27；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-054-f6aa729.json；log_path = out/audit-20260930/qa/ctrl-ag054-f6aa729.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E04; sections 7.2 L819 and 7.2.1 L827/L831, E04 L1239/L1243; V008/V086/V096 subset: known pre-dispatch causes, versioned durable refusal replay, legacy API mapping and full-fence cleanup limited to Reserved; unknown cost remains charged/reserved, empty-source legacy retry retains stored sources
历史原实际结果（原时点）：actual_result = controller independent full re-run at f6aa729: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; broker 21 passed / 0 failed; workspace 89 result lines, 1195 passed / 0 failed; checker unittest 48 passed; two controller probes pass on head and both fail on actual parent; AG054 parent probes fail after successful compilation, exercising typed identity and cleanup-error behavior
历史原风险与不可声明：risk = No generic database/commit-failure classification, no unknown-cost refund; target execution error tested through private helper, not real process fault. No real provider bills, sandbox, complete recovery, exhaustive concurrency, full E/V, effect or release
历史原回滚约束：rollback = revert PR #104 through an independent revert PR; accepted-head parent ba01a2e8489ceef6d1350f0babdac46d816071ec; retain immutable evidence, frozen plans, costs and revocation state
**历史first阶段（Codex原记录L100–110；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-054 / PR #104 — 确定派发前拒绝与安全清理预留
历史first事实：- 卡AG-054.md SHA256d167cdaa3c34fc28ad07089d65f1764d3367f5e063c8708ce44b6eae4d99ad0e；R1 ae9c3c1a5d7eba66ec755ebfcd35f50cfd366cd24d2837947f2bf927cf80c320；R2 30db20fa59bf3217fb13245286df618550df58dc60145bf0412f225d58537e69。
历史first事实：- 原基线3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360→交付7aee89b6ccb8160b7bf4b848dbc6c3bf0464c517→叠至ba01a2e8489ceef6d1350f0babdac46d816071ec→验收f6aa729a467f10f8b15b2d7b20e9a9d1c7d80351；merged_sha9b08c4b2fd882abab65f48ea0af8f23b13a3b50d，mergedAt2026-10-01T13:34:48Z，双方tree437240ad205e1009d2dc51681e20d39b9ef93a6c。
历史first事实：- fixed f6aa729a467f10f8b15b2d7b20e9a9d1c7d80351，父ba01a2e8489ceef6d1350f0babdac46d816071ec；原交付7aee89b6…单提交rebase patch逐字节cmp0。9文件+3341/-219全diff核验，R1旧空来源恢复/R2两旧撤销断言严格授权范围，无产品返修/冲突。
历史first事实：- 全新target-ctrl-ag054，ctrl-ag054-f6aa729.log*全部8项exit0，1195/0=1158+37、84二进制+5doc组，Python48。原实施全部证据qa/impl-ag054（含6项基线行为失败、37新测、沙箱/格式/夹具错误），无真实DBcommit故障注入。
历史first事实：- root append SHA256583e09942bd40c84233d31027fb74d0bbc4891aa6c965c1dc9e5e620932f01bd；冻结broker父源a22204b59752fd87d37d4bba008774ed694c08819bb9997f3fb7364bca907e4d、dev父源eb699ef581e9e23a15ca3376c58effebdae964241532d3a184acf6606093660b。head ctrl-ag054-adversarial-head-f6aa729.log2/0 exit0；实际父ctrl-ag054-adversarial-base-ba01a2e.log0/2 exit101（编译成功），clean。
历史first事实：- P1 early-read之后他请求同id真实dispatch→Uncertain，head InvalidRequest固定码，父Unauthorized；完整旧call/root及20micro/transport0不变。P2 runner旧Reserved快照后真实call变Uncertain再clock失败，head传播cleanup Conflict，父Invalid时钟错误；完整call/root及1micro/无新receipt双方都保持。P2不冒称基线误退款。首次探针同步闸超时非产品失败，已保留；仅block_in_place调度修正，两树同探针。
历史first事实：- verified限E04/§7.2L819/§7.2.1L827,L831/E04L1239,L1243，V008/V086/V096确定拒绝/真实持久原因与重放/有效fence仅Reserved收口/legacy兼容/未知费用。R1空来源不擦记录，不宣称每次显式完整列表；generic DB/commit/坏引用不猜typed cause，不退款。
历史first事实：- 不可声明真实provider/账单/沙箱/Recovery/整体E-V/并发穷举/效果/发行；target Err仅私有helper注入。回滚代码参考父ba01a2e8489ceef6d1350f0babdac46d816071ec但保留账单/撤销事实。原发布auto-review拒绝经root直接人类授权/精确payload复核获批，无绕过。下一步fail-fast固定head merge。
历史first事实：- admin=false，无冲突；产品返修0、范围/兼容裁决2。所有自测原失败、自动审批拒绝/授权复核成功、root探针初次调度错误如台账，QA保留。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E04；E04.ag054_typed_predispatch [withdrawn，旧通过不计当前，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-054.md`，文件SHA256 `d167cdaa3c34fc28ad07089d65f1764d3367f5e063c8708ce44b6eae4d99ad0e`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag054-f6aa729.log.test`，文件SHA256 `9b4b1cb42f2fb6ad96c04aa85eb316aa8d78ce46fea9d5e9f5ebf6df850af59d`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag054-adversarial-head-f6aa729.log`，文件SHA256 `61aec73ab08bdf1b054fdda6b4fc5f61e775fbb43382b85621efed1559f2cc04`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag054-adversarial-base-ba01a2e.log`，文件SHA256 `cf265ff26f7059f8f32e0e36e9c3714db5d28f1d3e8322a805670e1a2e9ebcc8`。
### PR #105 — AG-058（第四轮历史）
冻结M6角色：公开head 4edb5b79220fe443d700808b99da9b9f6590a910；本地验收head 4edb5b79220fe443d700808b99da9b9f6590a910；实际merge 5337f4b17558b4c95176885d39f2518142d965b9；mergedAt 2026-10-01T13:45:01Z。
合并父 9b08c4b2fd882abab65f48ea0af8f23b13a3b50d, 4edb5b79220fe443d700808b99da9b9f6590a910；tree 1a8390115bf882369775302bdba3be8245256a47。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E14/verified_subscopes/2`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E14.ag058_current_description_boundaries；description = Current CLI, meta and cleanup description boundaries
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 4edb5b79220fe443d700808b99da9b9f6590a910；pr = https://github.com/acosmi/RSIAgent/pull/105；merged_sha = 5337f4b17558b4c95176885d39f2518142d965b9；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/meta_trial_v42.rs；command = cargo test --locked --offline -p evo-engine --test meta_trial_v42；exit_code = 0
历史原证据登记：input_digest = bc5e95958c32afc8a7dccf863ebd175c69efd1811e0f44804c57cf376dc20b3d；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-058-4edb5b7.json；log_path = out/audit-20260930/qa/ctrl-ag058-4edb5b7.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E00/E02/E08/E13/E14/E15 description-only correction; sections 6.1 L605-L609, 9 L1009/L1013, 11 L1077, E08 L1295/L1299 and E13 L1365/L1369; V001/V002/V017/V032/V033/V035/V037 subset. Text matches current consumers; three Rust files differ only in documentation comments. Counter depletion preserves meta registration fingerprint while immutable cost contract changes do not
历史原实际结果（原时点）：actual_result = controller independent full re-run at 4edb5b7: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; meta_trial_v42 29 passed / 0 failed; workspace 89 result lines, 1195 passed / 0 failed; checker unittest 48 passed; two controller probes pass on head and actual parent; both probes are explicit compatibility/claim boundaries, not old runtime bug reproduction
历史原风险与不可声明：risk = No new runtime capability or complete E/V verification; old MetaEvidence is caller-reported, spends may differ, no independent attestation or per-stream hard cap. QualityStatus remains Unknown; drift/retention are not automatic; no general cleanup, effect or release claim
历史原回滚约束：rollback = revert PR #105 through an independent revert PR; accepted-head parent 9b08c4b2fd882abab65f48ea0af8f23b13a3b50d; retain immutable evidence, frozen plans, costs and revocation state
**历史first阶段（Codex原记录L114–124；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-058 / PR #105 — 当前行为文字说明
历史first事实：- 卡cards/AG-058.md SHA256105adddba27b6675df2772c6d8e3e6fecf8344970eb4e00667a416c150137b4a；无裁决。
历史first事实：- 原基线35b4d4e08df121e68467b0783b29303e9fa31d94，merged_sha5337f4b17558b4c95176885d39f2518142d965b9，mergedAt2026-10-01T13:45:01Z，tree1a8390115bf882369775302bdba3be8245256a47与验收head一致。
历史first事实：- fixed 4edb5b79220fe443d700808b99da9b9f6590a910，父9b08c4b2fd882abab65f48ea0af8f23b13a3b50d。交付cdae99de5bcd66b32779c8c16d92997a8f2ba120从35b4d4e叠放，唯一patch逐字节cmp0；5文件六授权段+26/-24全部diff和实际消费者独立复核。三Rust去文档注释后剩余字节相同（ctrl-ag058-comment-only-proof.json），无产品/测试体改动。
历史first事实：- 全新target-ctrl-ag058，ctrl-ag058-4edb5b7.log*全部8项0，1195/0=父1195+0，84二进制+5doc组，Python48。原实施1150/0基于旧基线，首次1145/5 HTTP沙箱失败保留，所有scratch已归qa/impl-ag058。
历史first事实：- probe-ag058冻结source-meta-trial-ba01a2e.rs SHA256e37ec685f923873ba3cc274198ab6157f3f5ca908af1192e477cbe6f5049cc3a；append.rs SHA2565b853d7a119c49b139fd589243e3a0b37ef695b2d2e4514fd1bdf2a50993e803。head ctrl-ag058-adversarial-head-4edb5b7.log与实际父ctrl-ag058-adversarial-base-9b08c4b.log都2/0、exit0、clean。P1预算消耗counter不改注册指纹、成本合同变化会改；P2不等实耗/未验证身份是旧Meta报送，伪造验证字段拒绝，QualityStatus仍只Unknown。明确是兼容/声明边界，非产品缺陷复现。
历史first事实：- verified限E00/E02/E08/E13/E14/E15现状说明，§6.1L605–609/§9L1009,L1013/§11L1077/E08L1295,L1299/E13L1365,L1369，V001/V002/V017/V032/V033/V035/V037子范围；不可声明新能力/全部E-V/独立MetaEvidence验证/每流硬预算/自动保留漂移/全清理schema并发/效果/稳定性/发行。回滚仅5处文字，参考父9b08c4b2fd882abab65f48ea0af8f23b13a3b50d。
历史first事实：- 返修0、裁决0、无冲突。子代理push自动审批拒绝root依直接人类授权/实际5文件payload/目的地重审获准，首TLS原请求重试；栈位正文EOF重试成功。全部fail-fast前置通过后固定head合并成功。
历史first事实：- admin=false，返修0，无冲突。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E00/E02/E08/E13/E14/E15；E14.ag058_current_description_boundaries [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-058.md`，文件SHA256 `105adddba27b6675df2772c6d8e3e6fecf8344970eb4e00667a416c150137b4a`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag058-4edb5b7.log.test`，文件SHA256 `2cfe5cc98208bdae888e940bbd40dcccb791d940aa283ee70b8e2d417613714c`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag058-adversarial-head-4edb5b7.log`，文件SHA256 `790b46895b97247ccb95f6f7c738152d99b7d134c4e18a3076aa52722ec76ade`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag058-adversarial-base-9b08c4b.log`，文件SHA256 `0b0421e088fae666bfe1bf66421b848812a9284025ef861e62513b07bab64f01`。
### PR #106 — AG-057（第四轮历史）
冻结M6角色：公开head 9150d8807d107f1ad24daa59b90f7de14f092477；本地验收head 9150d8807d107f1ad24daa59b90f7de14f092477；实际merge 279ee7c5290fabbb20cbd16b7e53c5ac2b71c188；mergedAt 2026-10-01T14:03:35Z。
合并父 5337f4b17558b4c95176885d39f2518142d965b9, 9150d8807d107f1ad24daa59b90f7de14f092477；tree c7b769978932482051a419ba8f227b579ceaef0d。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E01/verified_subscopes/1`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E01.ag057_explicit_sample_registration；description = Explicit sample input and evaluation preregistration requirements
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 9150d8807d107f1ad24daa59b90f7de14f092477；pr = https://github.com/acosmi/RSIAgent/pull/106；merged_sha = 279ee7c5290fabbb20cbd16b7e53c5ac2b71c188；pr_state = merged
历史原测试登记：test_entry = crates/evo-core/tests/evaluation_contract_v42.rs；command = cargo test --locked --offline -p evo-core --test evaluation_contract_v42；exit_code = 0
历史原证据登记：input_digest = ec5ad0a4d9bf2f53521fce35f9735b2ab22b62267861715e16b14e4199590507；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-057-9150d88.json；log_path = out/audit-20260930/qa/ctrl-ag057-9150d88.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E01; sections 3.1-3.6 L239-L401 and E01 L1197/L1201; V010/V011/V012/V013/V084/V085/V096/V097 limited to static registration obligations, explicit n input, legacy v1 bytes/digests and ticket-binding regression. first_low_risk(id,n) requires the supplied count and preserves the existing 2..100000 validation; fifteen old fixtures pass explicit 60
历史原实际结果（原时点）：actual_result = controller independent full re-run at 9150d88: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; evaluation_contract_v42 7 passed / 0 failed; workspace 90 result lines, 1202 passed / 0 failed; checker unittest 48 passed; two controller probes pass on head and actual parent; both probes are explicit compatibility/claim boundaries, not old runtime bug reproduction
历史原风险与不可声明：risk = Intentional Rust source API break with migration guidance, unchanged wire/schema/statistics/budget policy. Legal n is not a power or independence proof; no real task/oracle/anchor registration, paid authority, nonzero-budget policy fix, full E01/V, benefit or release; QueryBook boundary is in-memory
历史原回滚约束：rollback = revert PR #106 through an independent revert PR; accepted-head parent 5337f4b17558b4c95176885d39f2518142d965b9; retain immutable evidence, frozen plans, costs and revocation state
**历史first阶段（Codex原记录L128–139；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-057 / PR #106 — 显式样本量与实验登记说明
历史first事实：- 卡cards/AG-057.md SHA2562af16306712654bb75a261abbeaae1f1a49c4155327412a39b9fe007cec2a4b8，无裁决。
历史first事实：- 原基线ba01a2e8489ceef6d1350f0babdac46d816071ec；merged_sha279ee7c5290fabbb20cbd16b7e53c5ac2b71c188，mergedAt2026-10-01T14:03:35Z，双方treec7b769978932482051a419ba8f227b579ceaef0d。
历史first事实：- 固定head9150d8807d107f1ad24daa59b90f7de14f092477，父5337f4b17558b4c95176885d39f2518142d965b9。交付2121002bbe54d902a9e4810a819cc980c7c32d52从ba01a2e叠放，单提交patch逐字节cmp0、literal full lease成功，无冲突。12文件+406/-44全部审阅；构造器显式n、15旧fixture只补60、新7测试、文档登记要求，wire/算法/预算规则未改。
历史first事实：- 全新target-ctrl-ag057，qa/ctrl-ag057-9150d88.log*全部8项exit0；1202/0=1195+7，85二进制+5doc组，Python48。实施旧基线1165/0、首次HTTP1160/5、首次采集器行数误断言/格式/索引锁失败完整在qa/impl-ag057。旧helper实际运行产n60，另手动n137后freeze，未伪造旧二参API运行。
历史first事实：- 主控probe-ag057/source-core-golden-c0ea35d.rs SHA256b24ae22a8dfcf2c8aa8ee7b60ea525d8f00fec99516b04c85ca3032a59b7780a，append.rs SHA2565a22b53c72a91ec6586d150a733496ac3cd66e628eb618c83649a397787bb4b4。head ctrl-ag057-adversarial-head-9150d88.log与实际父ctrl-ag057-adversarial-base-5337f4b.log均2/0 exit0、clean。P1 null/字符串/负数/小数/布尔/数组/缺失/重复及Unicode转义重复n拒绝，语义越界仍需validate（serde本身不负责）；P2改变n使摘要变化，原/新QueryBook互拒另一计划票据，重复/取消不退尝试。两树同过明确兼容边界，不冒称旧单参API忽略传入n的运行缺陷。
历史first事实：- verified仅E01 §3.1–3.6 L239–401/E01L1197,L1201，V010/V011/V012/V013/V084/V085/V096/V097静态登记、显式样本量输入和旧wire/查询绑定子范围。源码API明确破坏性first_low_risk(id,n)，提供迁移；n2..100000只是合法界不证明独立性/功效，60不代表正式样本要求。
历史first事实：- 不可声明首任务/oracle/锚点真实冻结、付费许可/真实效果、全部E01/V、生产profile/非零预算已授权/新统计算法/可发行。QueryBook仍是现有内存边界；公开字段没有因本卡变为不可变，混票拒绝不等于任意篡改持久冻结计划的全链保护。GC22保持未修。回滚参考父5337f4b并保留冻结记录/费用。
历史first事实：- 返修0、裁决0，无冲突，无auto-review拒绝；执行环境localhost/index锁获准后同项重跑。全部fail-fast前置成功后已固定head合并。
历史first事实：- admin=false，无冲突，返修0。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E01；E01.ag057_explicit_sample_registration [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-057.md`，文件SHA256 `2af16306712654bb75a261abbeaae1f1a49c4155327412a39b9fe007cec2a4b8`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag057-9150d88.log.test`，文件SHA256 `9a9493b53dedd500d4812945cbae1d7853a453cf7a2c49fc2599424c882d2ad1`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag057/source-core-golden-c0ea35d.rs`，文件SHA256 `b24ae22a8dfcf2c8aa8ee7b60ea525d8f00fec99516b04c85ca3032a59b7780a`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag057-adversarial-head-9150d88.log`，文件SHA256 `0fcac93ad17e31e1aa4aa1d1f08905b6f82b5fc9fea0450d1da3d91e0515231f`。
### PR #107 — AG-063（第四轮历史）
冻结M6角色：公开head 1bf70d3cc51e9a5ecf3eb1314de2221abf32042c；本地验收head 1bf70d3cc51e9a5ecf3eb1314de2221abf32042c；实际merge 3f9ed595bdcd8bf241bc73fb83640d277947810a；mergedAt 2026-10-01T14:24:58Z。
合并父 279ee7c5290fabbb20cbd16b7e53c5ac2b71c188, 1bf70d3cc51e9a5ecf3eb1314de2221abf32042c；tree c3a5417f233ff083b90eee137338ee8303ad5d1a。原双父合并角色；不是本轮网络重查或重新执行。
**历史first阶段（Codex原记录L143–151；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-063 / PR #107 — 第二批台账与索引
历史first事实：- 卡cards/AG-063.md SHA2560e98fc7424b7d0d538897ffa9008051c1889e953ec7800147d0539c39a758637，无裁决。基线279ee7c5290fabbb20cbd16b7e53c5ac2b71c188；merged_sha3f9ed595bdcd8bf241bc73fb83640d277947810a，mergedAt2026-10-01T14:24:58Z，双方treec3a5417f233ff083b90eee137338ee8303ad5d1a。
历史first事实：- 固定head1bf70d3cc51e9a5ecf3eb1314de2221abf32042c，父279ee7c5290fabbb20cbd16b7e53c5ac2b71c188，无叠放。3白名单文件+285/-15全部复核；原事件折入仅标题升层，fold SHA256ef9be7309881bdf01643b1fd966e2cc5589c75123d7ab6791068a1506d3d6a2a；所有旧verified记录相等。
历史first事实：- 全新target-ctrl-ag063，qa/ctrl-ag063-1bf70d3.log*八项exit0；1202/0=父1202+0，85二进制+5doc组，Python48；定向ctrl-ag063-support-scope-1bf70d3.log14/0。
历史first事实：- 主控probe-ag063/probe.py SHA2564f762d85fa4fa3f3d40554db343b750b5fff8a36b3d664139bc233b0ebc19d18；head ctrl-ag063-adversarial-head-1bf70d3.log全部通过，实际父ctrl-ag063-adversarial-base-279ee7c.log缺四记录exit1，历史篡改/正向/clean通过。四项source/merge两父/tree/祖先/testblob/input digest/日志hash与真实计数逐一核实。T0旧AG042输入、T1新AG055输入、T2跨scope日志替换、T3错merge、T4新AG057输入均拒绝；历史篡改与结构拒绝为已有边界，父登记失败是覆盖缺口，非产品缺陷。两树同方法复制本地受测input并finally删除，tracked clean。
历史first事实：- verified仅E00/E16.6，E00L1181–1189/E16.6L1459–1463/§18.5L2162–2166，V001/V071/V072/V073/V074/V080/V098四项已验子范围登记与可追溯。AG055/054/058/057各自原验证范围和失败证据保留，不扩为完整E/V。E01仍blocked、GA14阶段计划绑定/GA23文件系统政策缺合同、E03诊断真实证据缺口和056workflow权限阻塞明确。
历史first事实：- 不可声明新产品、历史包归并、完整默认范围、真实效果或发行；旧历史部分input缺失保留。回滚参考父279ee7c并保留证据/费用/撤销事实。无返修、无冲突；按启动§5索引任务采用Python反例而非Rustappend。首次本地index.lock权限失败获准后正常提交，未有auto-review拒绝。
历史first事实：- admin=false，返修0，无冲突。主控本地脚本生成器SHA出现次数误断言先于任何远端操作失败，纠正后才生成执行；API EOF/TLS均在fail-fast重试内成功后继续。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E00/E16.6；未入当前98单目标索引：台账/索引PR。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-063.md`，文件SHA256 `0e98fc7424b7d0d538897ffa9008051c1889e953ec7800147d0539c39a758637`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag063-1bf70d3.log.test`，文件SHA256 `f401cfdc980476f7283abe28a8229a57a3381edb75e2bf173737a21e118b2a62`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag063/probe.py`，文件SHA256 `4f762d85fa4fa3f3d40554db343b750b5fff8a36b3d664139bc233b0ebc19d18`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag063-adversarial-head-1bf70d3.log`，文件SHA256 `ee09e88c2065f600edce7522c5f97b9ebe8095c7746e45a61b65a9dfa76407b7`。
### PR #108 — AG-062（第四轮历史）
冻结M6角色：公开head e6e7eec47a3494425a1842604c154d8695089222；本地验收head e6e7eec47a3494425a1842604c154d8695089222；实际merge 6f0e5fa35b582f0f4cde4207ddf59d65a51b180a；mergedAt 2026-10-01T14:36:56Z。
合并父 3f9ed595bdcd8bf241bc73fb83640d277947810a, e6e7eec47a3494425a1842604c154d8695089222；tree 2612aa2fe849986b20eeffe43a4ff759bde6eae7。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（历史已撤出对象（不计当前98；原错误/旧复核标签仍保留））：`out/audit-20260930/qa/codex-r7-ag124/withdrawn-records.json`，JSON pointer `/records/15/record`，文件SHA256 `d71db404637255b0600c8dcdeae8ac3725cad9360e3cb65e51f558da368c19fd`。
历史原登记身份：id = E02.ag062_required_improver_baselines；description = Required baseline labels at independent Improver compilation
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = not_affected
历史原提交角色：source_sha = e6e7eec47a3494425a1842604c154d8695089222；pr = https://github.com/acosmi/RSIAgent/pull/108；merged_sha = 6f0e5fa35b582f0f4cde4207ddf59d65a51b180a；pr_state = merged
历史原测试登记：test_entry = crates/evo-core/tests/improver_required_baselines_v42.rs；command = cargo test --locked --offline -p evo-core --test improver_required_baselines_v42；exit_code = 0
历史原证据登记：input_digest = ad35eaa9900b9a03fd40ab70d40e0d46e59c85d34b6dec62f73f8667b5b7b854；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-062-e6e7eec.json；log_path = out/audit-20260930/qa/ctrl-ag062-e6e7eec.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E02; sections 5.3 L473/L487-L491 and 5.1 L453/L455, E02 L1211/L1215; V046 missing P/B labels and V043 legal three-state compatibility: both nonempty labels are required after the original parent/baseline checks, even for all-Set patches
历史原实际结果（原时点）：actual_result = controller independent full re-run at e6e7eec: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; improver_required_baselines_v42 13 passed / 0 failed; workspace 91 result lines, 1215 passed / 0 failed; checker unittest 48 passed; two root probes pass on head and both fail after successful compilation on the actual parent; 13 new core tests
历史原风险与不可声明：risk = Does not authenticate P/B contents, digests, approval, existence, revocation or profile binding; no full FieldContract or consumer coverage, real host, effect or release
历史原回滚约束：rollback = revert PR #108 through an independent revert PR; accepted-head parent 3f9ed595bdcd8bf241bc73fb83640d277947810a; retain immutable evidence, frozen plans, costs and revocation state
**历史first阶段（Codex原记录L155–164；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-062 / PR #108 — 独立Improver缺失基线上下文拒绝
历史first事实：- 卡AG-062.md SHA256792d845e7a7813f4b55d29840080da5a8e7fe683f48617a0b854f9d73bee8814，R1 SHA256b6c6cb8910d172229595dc76f6891c33766d95a08e9a77b1ec7f9407cc5b4c67；merged_sha6f0e5fa35b582f0f4cde4207ddf59d65a51b180a，mergedAt2026-10-01T14:36:56Z，双方tree2612aa2fe849986b20eeffe43a4ff759bde6eae7。
历史first事实：- 交付5d70466ce8b90164ddc14bd120fb20a8fcd3cb22，原基线9b08c4b2fd882abab65f48ea0af8f23b13a3b50d；叠至3f9ed595bdcd8bf241bc73fb83640d277947810a→固定验收e6e7eec47a3494425a1842604c154d8695089222。唯一提交前后patch逐字节cmp0，无冲突；literal fullref/oldhead lease首次TLS失败原请求重试成功。2白名单文件+460/-0全部已读，产品只compile_improver入口6行，新测试454行13项。
历史first事实：- 全新target-ctrl-ag062，qa/ctrl-ag062-e6e7eec.log*八项exit0；1215/0=1202+13，86二进制+5doc组；Python48。self旧基线1208/0；81个缺P/B组合实际接受导致baseline10/3，去P/去B变异各12/1，恢复13/0。四完整产物JSON/所有fingerprint逐字相同。原clippy循环告警及日志parser漏1标记的初错、修正、auto-review拒绝均保存在qa/impl-ag062。
历史first事实：- root probe-ag062/source-core-golden-c0ea35d.rs SHA256b24ae22a8dfcf2c8aa8ee7b60ea525d8f00fec99516b04c85ca3032a59b7780a；append.rs SHA2561f4abf5cd3476056bf115b013874b3db4f9c71d3850719e9c3570cdb8e85ce54。head ctrl-ag062-adversarial-head-e6e7eec.log2/0 exit0，实际父ctrl-ag062-adversarial-base-3f9ed59.log0/2 exit101且正常编译；双方探针后clean。P1真实JSON往返、空id、evolution真假、全Set及重复调用缺P/B仍拒，非空NUL/tab标签继续旧允许；P2缺上下文先于无效Set输出值拒绝，合法标签时旧值错误不变。父P1错误接受，P2返回补丁值错误而未核上下文，非编译失败。
历史first事实：- verified仅E02，§5.3L473/L487–491、§5.1L453/L455、E02L1211/L1215；V046缺P/B基础拒绝与V043三态合法兼容子范围。保留parent/baseline Strategy原错误优先，所有三态含全Set均需两个非空标签。R1纠正原卡空instruction措辞，现有Strategy空/白/NUL等拒绝不变。
历史first事实：- 不可声明真实P/B内容摘要/批准/存在/撤销/profile绑定、非空标签真实性、完整FieldContract或未实现max执行消费者、全部E02/V046、真实宿主/效果/发行。旧v1结构/字节/算法不变。回滚仅移除门和新测试，参考父3f9ed59且保留历史/费用事实。
历史first事实：- 产品返修0，R1测试口径裁决1；无冲突。子代理push自动审批拒绝后冻结；root完整payload/remote/直接人类指定哈希文件§3.4/§10复核，限定同请求新证据审批获准后发布，原拒绝保留，未绕过。主控全部独立验证通过后才ready/固定head合并。
历史first事实：- admin=false，无冲突，产品返修0/R1一次；合并所有硬前置成功。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E02；E02.ag062_required_improver_baselines [withdrawn，旧通过不计当前，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-062.md`，文件SHA256 `792d845e7a7813f4b55d29840080da5a8e7fe683f48617a0b854f9d73bee8814`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag062-e6e7eec.log.test`，文件SHA256 `7b728807816be81997922350cbf5c81fda4d5262be6d7df1bfab5eb25e478273`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag062/source-core-golden-c0ea35d.rs`，文件SHA256 `b24ae22a8dfcf2c8aa8ee7b60ea525d8f00fec99516b04c85ca3032a59b7780a`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag062-adversarial-head-e6e7eec.log`，文件SHA256 `d38c69044cb09f9385f8921856c2ebbbf654df495c2a5c3ccec58136d2cf72c0`。
### PR #109 — AG-064（第四轮历史）
冻结M6角色：公开head 437af3fc7e414a3509bbb3f9d2cc8a614f231616；本地验收head 437af3fc7e414a3509bbb3f9d2cc8a614f231616；实际merge c6e13e906ea888916294e6deac77c37f4391225f；mergedAt 2026-10-01T15:32:43Z。
合并父 eb275812f09dd8850ad4edac08a2282cb5e82112, 437af3fc7e414a3509bbb3f9d2cc8a614f231616；tree 13009e062ea45a967c8fd6be5e329a5b3d9c9f53。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（历史已撤出对象（不计当前98；原错误/旧复核标签仍保留））：`out/audit-20260930/qa/codex-r7-ag124/withdrawn-records.json`，JSON pointer `/records/7/record`，文件SHA256 `d71db404637255b0600c8dcdeae8ac3725cad9360e3cb65e51f558da368c19fd`。
历史原登记身份：id = E10.ag064_replay_episode_recovery；description = Replay recovery facts, lineage eligibility and batch episode bound
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = not_affected
历史原提交角色：source_sha = 437af3fc7e414a3509bbb3f9d2cc8a614f231616；pr = https://github.com/acosmi/RSIAgent/pull/109；merged_sha = c6e13e906ea888916294e6deac77c37f4391225f；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/replay_recovery_contract_v42.rs；command = cargo test --locked --offline -p evo-engine --test replay_recovery_contract_v42；exit_code = 0
历史原证据登记：input_digest = 875f78f9d6b8d4b97c772edda20d59d8ad7265c486eb306eb3f1397b6a1099b1；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-064-437af3f.json；log_path = out/audit-20260930/qa/ctrl-ag064-437af3f.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E09/E10; sections 7.1 L785/L787, 7.1.1 L791-L809, 7.2.1 L825-L831, E10 L1323-L1329, 5.1 L453 and 7.4.1 L853-L855; V024/V081/V086.b/d subset: derive per-episode counters from observed Recover targets, project after the whole batch, reject exhausted failure lineages and duplicate episodes in a batch
历史原实际结果（原时点）：actual_result = controller independent full re-run at 437af3f: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; replay_recovery_contract_v42 10 passed / 0 failed; workspace 95 result lines, 1263 passed / 0 failed; checker unittest 48 passed; two root probes pass on head and both fail after successful compilation on the actual parent; 4 new core and 10 new engine tests; three actual old-program report fixtures and unchanged stored-history assertions
历史原风险与不可声明：risk = No production repair templates/reset/leases/billing, arbitrary-prefix authentication, full legal-action filtering, normal Deepen expanded-parent rule or PolicyStop classification. Old affected reports remain statically readable but semantic verification and same-ID reruns reject; no migration, overwrite, full E/V, effect or release
历史原回滚约束：rollback = revert PR #109 through an independent revert PR; accepted-head parent eb275812f09dd8850ad4edac08a2282cb5e82112; retain immutable evidence, frozen plans, costs and revocation state
**历史first阶段（Codex原记录L181–190；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-064 / PR #109 — 回放恢复事实与同批episode上限
历史first事实：- fixed head437af3fc7e414a3509bbb3f9d2cc8a614f231616，merged_shac6e13e906ea888916294e6deac77c37f4391225f，mergedAt2026-10-01T15:32:43Z；accepted/merged tree=13009e062ea45a967c8fd6be5e329a5b3d9c9f53，实际父eb275812f09dd8850ad4edac08a2282cb5e82112+验收head相等。全部fail-fast前置成功，admin=false，main已ff-only。产品返修0，R1环境1，无冲突。#107后产品3，触发AG067台账；清理随后，QA保留。
历史first事实：- 交付d8641b28f9fdd354c50302625337edf10369b9b9原基线6f0e5fa35b582f0f4cde4207ddf59d65a51b180a，单提交叠至eb275812f09dd8850ad4edac08a2282cb5e82112→固定437af3fc7e414a3509bbb3f9d2cc8a614f231616；before/after-rebase.patch逐字节cmp0，无冲突，literal完整oldhead/fullref lease成功。6白名单文件+1159/-48完整diff已审，只有2产品文件与2新/2授权旧测试。
历史first事实：- 全新target-ctrl-ag064，qa/ctrl-ag064-437af3f.log*八项exit0，1263/0=1249+14，90测试二进制+5doc组，Python48。实施完整report归qa/impl-ag064/report.md SHA25632b37883ec899630f8b6cd2c1075974b9fd4f76f7a2bcaddc8c37406f4ac1325；原capture借用编译错、缺baseline来源夹具错、Stored缺Debug编译错均已读/保留，不计产品失败；最终1229/0和R1前1224/5保留。
历史first事实：- root冻结probe-ag064/source-replay-v41-6f0e5fa.rs SHA256c21745c6c21ef0dc8856923ad33a867f9b3a86f3348040dd9973bec368658c31；append.rs SHA256103a95c6b019612abbd31c198d43c61da077d3e960d927df9abfb7f686b0201f。head ctrl-ag064-adversarial-head-437af3f.log2/0 exit0，实际父ctrl-ag064-adversarial-base-eb27581.log0/2 exit101，正常编译且两树clean。P1公平首选14的episode必须在填槽前占用，W2/W4、反序/opaque改名排除11仍填12/13/15并核总cost；父选择同episode11。P2真实W4回放两个alias对、Observed Cancelled/UsageUncertain，必须只选5/7、6节点6探测6成本且共享counter1、unknown为Censored；父选4次Recover而8节点/8成本。世界原字节双方不变。
历史first事实：- verified仅E09/E10，§7.1L785,L787/§7.1.1L791–809/§7.2.1L825–831/E10L1323–1329/§5.1L453/§7.4.1L853–855；V024、V081、V086.b/d限可信前缀失败谱系、同批episode上限、本次已观察Recover事实派生与全批投影。旧status counter不当事实；Observed取消/未知消耗一次，OOS/缺历史/显式Censored旧路径不伪造dispatch。
历史first事实：- 三份真实旧程序生成合成报告完整字节已审，affected SHA64b34091cdbd9b520257d526153257e164bd9db247f332ebfde0990fda5e1137 static load可读但engine重算和重复同ID拒绝、不覆盖；正常Valid/Hard旧报告保持verified/幂等。schema/engine版本/ID不变，新旧同ID不可用覆盖当迁移。已有job元数据不等于评分重验。
历史first事实：- 不可声明真实repair模板/reset/lease/费用生命周期、任意伪造prefix防护、跨根episode身份关联、正常Deepen已展开父/一般available_actions/PolicyStop分类、旧报告迁移、完整E09/E10/V或效果/发行。原AG042顶部旧按node计数注释在授权两函数之外，明确仍有过期文字；本卡未改。回滚参考实际父eb27581，保留世界/不可变报告/费用/撤销证据。
历史first事实：- 卡AG-064.md 9cadb4054a444c15e83ae90530bc836b4ebb9ea6c97831987a174707d06301a5；R1环境6bae9206616698fa1f680646ccb4a156b2a8bfbcc01f459fdf14e217d16a9fdb；产品返修0、环境裁决1，无冲突。子chat外发审批拒绝后冻结，root以本chat直接人类哈希指令/六文件实际payload/remote新事实复核同请求获准后建draft#109；原拒绝和TLS失败保留。下一步fail-fast全文body读回/当前父/head/clean后固定head合并。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E09/E10；E10.ag064_replay_episode_recovery [withdrawn，旧通过不计当前，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-064.md`，文件SHA256 `9cadb4054a444c15e83ae90530bc836b4ebb9ea6c97831987a174707d06301a5`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag064-437af3f.log.test`，文件SHA256 `cf5ed572a49c35f95ca82ee4580d4a64420a47a62b10830f7b355ca6402b276a`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag064/source-replay-v41-6f0e5fa.rs`，文件SHA256 `c21745c6c21ef0dc8856923ad33a867f9b3a86f3348040dd9973bec368658c31`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag064-adversarial-head-437af3f.log`，文件SHA256 `95bb6b8cfb97f28ec878ab780a127e8d7bd8f085da293c9906a4720bdc90bfcf`。
### PR #110 — AG-065（第四轮历史）
冻结M6角色：公开head 2787e0c19f4b73298e73c0ddead0f959ceb6bfef；本地验收head 2787e0c19f4b73298e73c0ddead0f959ceb6bfef；实际merge eb275812f09dd8850ad4edac08a2282cb5e82112；mergedAt 2026-10-01T15:22:30Z。
合并父 6f0e5fa35b582f0f4cde4207ddf59d65a51b180a, 2787e0c19f4b73298e73c0ddead0f959ceb6bfef；tree b95696ec74a88f2fa4fbe2ae42baccf36f973908。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E02/verified_subscopes/2`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E02.ag065_host_surface_name_coverage；description = Host surface reverse name coverage
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 2787e0c19f4b73298e73c0ddead0f959ceb6bfef；pr = https://github.com/acosmi/RSIAgent/pull/110；merged_sha = eb275812f09dd8850ad4edac08a2282cb5e82112；pr_state = merged
历史原测试登记：test_entry = crates/evo-core/tests/host_surface_name_coverage_v42.rs；command = cargo test --locked --offline -p evo-core --test host_surface_name_coverage_v42；exit_code = 0
历史原证据登记：input_digest = 45ffd891ec47b3a3a5cdbdd3f85fffae9e49dd0e7542bbb2b9aaa9a6ec9d244e；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-065-2787e0c.json；log_path = out/audit-20260930/qa/ctrl-ag065-2787e0c.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E02/E16.4; sections 5.5 L523-L529 and 5.6 L541, E02 L1215; V060 deleted/renamed-away names and V061 nonempty extraction truncation: each manifest name must occur in extracted names, across all three classifications, before registration or a new run
历史原实际结果（原时点）：actual_result = controller independent full re-run at 2787e0c: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; host_surface_name_coverage_v42 16 passed / 0 failed; workspace 93 result lines, 1249 passed / 0 failed; checker unittest 48 passed; two root probes pass on head and both fail after successful compilation on the actual parent; 16 new core and 18 new engine tests, same target basename but separately bound source blobs
历史原风险与不可声明：risk = Names only; no type/enum/default drift, real extraction/consumer authentication, Tool-only mapping or Claude evidence. Existing frozen run snapshot reuse is outside the new guard; no full E/V, effect or release
历史原回滚约束：rollback = revert PR #110 through an independent revert PR; accepted-head parent 6f0e5fa35b582f0f4cde4207ddf59d65a51b180a; retain immutable evidence, frozen plans, costs and revocation state
**历史first阶段（Codex原记录L168–177；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-065 / PR #110 — HostSurface名称反向覆盖
历史first事实：- fixed head2787e0c19f4b73298e73c0ddead0f959ceb6bfef，merged_shaeb275812f09dd8850ad4edac08a2282cb5e82112，mergedAt2026-10-01T15:22:30Z；accepted/merged tree=b95696ec74a88f2fa4fbe2ae42baccf36f973908，实际父6f0e5fa35b582f0f4cde4207ddf59d65a51b180a+验收head相等。全部fail-fast前置成功，admin=false（CLEAN无需覆盖保护），main已ff-only。产品返修0，R1环境1，无冲突。#107后产品2，清理随后，QA保留。
历史first事实：- 固定head2787e0c19f4b73298e73c0ddead0f959ceb6bfef，父6f0e5fa35b582f0f4cde4207ddf59d65a51b180a；无需叠放。3白名单文件+705/-0全diff已审、预审SHA与交付逐字相同；产品只10行反向名称门，新测试16+18。PR #110与WT一致且clean。
历史first事实：- 全新target-ctrl-ag065，qa/ctrl-ag065-2787e0c.log*八项exit0；1249/0=1215+34，88测试二进制+5doc组，Python48。实施最终报告qa/impl-ag065/AG-065-report.txt（09aa1f52acba01ea67d38a5047a5d875dea527c9a4fed72252df417a4229e55a）已完整读；原baseline15/19、移门15/19、最终34/0及沙箱1244/5均保留。
历史first事实：- root冻结probe-ag065/source-release-store-6f0e5fa.rs SHA256c01ea240e0646b6eeecb2e78db1a5707275b8080feee99a5bbac8c7f81119697；append.rs SHA256723dcb1d47833493f33730e8b25d67fa66f98a9268c7095f9c8cd158b549806d。head ctrl-ag065-adversarial-head-2787e0c.log2/0 exit0；实际父ctrl-ag065-adversarial-base-6f0e5fa.log0/2 exit101，均成功编译/事后clean。
历史first事实：- P1旧缺项record同ID JSON往返重复注册仍须拒；同ID改完整内容不可覆盖不可变记录，新ID完整换序正向成功。父错误Ok两次。P2跨namespace相同IDs、evolution真假各重复新prepare，坏scope均须精确拒MODEL且无run/artifact变化，好scope相同ID成功并幂等；父错误Ok四次且留下snapshot。root未声称这两个probe扫描audit；实施独立34项包含audit核验。
历史first事实：- verified仅E02/E16.4，§5.5L523–529/§5.6L541/E02L1215，V060名称删除或重命名导致旧名消失、V061非空截断子范围。全部旧校验后按manifest顺序首缺项，三个classification同等，精确case/Unicode，无去重/排序/schema/digest/错误优先级改变。
历史first事实：- 不可声明类型/枚举/默认值漂移、真实提取与完整性、消费者真实性、ToolOnly映射、真实Claude/全部E-V/效果/发行，已完成run冻结快照复用不在本门覆盖。回滚参考父6f0e5fa并保留既有记录/费用/证据。
历史first事实：- 产品返修0，R1环境裁决1（AG-065-R1.md eb6a9dbbcbbf2441b39afbd69cc2808711f884fa10eb2d3156cff007f14d2ffa），原卡2de54bf3ae1a50237c9a15cfa8551cd0c75f53c56dde1127008bc97a980f04d8；无冲突。子代理push自动审批拒绝后冻结，root核直接人类哈希指令/实际payload/远端public且当前admin-push权限新事实，同一限定请求获准后成功发布，原拒绝/首次TLS保留；报告写“未创建PR”是root接管前事实。合并前必须fail-fast正文逐字读回/fetch/head/parent/clean。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E02/E16.4；E02.ag065_host_surface_name_coverage [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-065-R1.md`，文件SHA256 `eb6a9dbbcbbf2441b39afbd69cc2808711f884fa10eb2d3156cff007f14d2ffa`。
- 原卡导航：`out/audit-20260930/cards/AG-065.md`，文件SHA256 `2de54bf3ae1a50237c9a15cfa8551cd0c75f53c56dde1127008bc97a980f04d8`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag065-2787e0c.log.test`，文件SHA256 `e2e35bed3bdf5385890bee820b6740bf671e267a30886b82e30f230cecca18cb`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag065/source-release-store-6f0e5fa.rs`，文件SHA256 `c01ea240e0646b6eeecb2e78db1a5707275b8080feee99a5bbac8c7f81119697`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag065-adversarial-head-2787e0c.log`，文件SHA256 `3dba2ca35c77c95607edcc76c77e0c03b9a37314588aca8753ce416c2b835141`。
### PR #111 — AG-066（第四轮历史）
冻结M6角色：公开head 1d0d93d1dc42deae83056bfc8602fd3bf2760f3d；本地验收head 1d0d93d1dc42deae83056bfc8602fd3bf2760f3d；实际merge 200c43bc19368ff52b064e67eb1b14e35f096d39；mergedAt 2026-10-01T16:11:27Z。
合并父 e7ec4904b42e8c6f0d20e425b3bdc77c3109c180, 1d0d93d1dc42deae83056bfc8602fd3bf2760f3d；tree 53426fe775001c01f8e09437c4a3d4e4b1faf80c。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E16.4/verified_subscopes/0`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E16.4.ag066_local_stdio_harness；description = Local MCP stdio process harness with frozen v1 protocol
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 1d0d93d1dc42deae83056bfc8602fd3bf2760f3d；pr = https://github.com/acosmi/RSIAgent/pull/111；merged_sha = 200c43bc19368ff52b064e67eb1b14e35f096d39；pr_state = merged
历史原测试登记：test_entry = crates/evo-mcp/tests/service.rs；command = cargo test --locked --offline -p evo-mcp --test service；exit_code = 0
历史原证据登记：input_digest = 9f982bc0afa115e267537cb1779beef852d788b7301cd263da811509e2385ff8；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-066-1d0d93d.json；log_path = out/audit-20260930/qa/ctrl-ag066-1d0d93d.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E02/E16.4; section 6.1 L605-L609 and E16.4 L1443-L1445; V002 local v1 interface and V039 platform/host claim boundaries: fixed SDK handshake, four schemas, persistence/reconnect and protocol rejection paths in local subprocesses
历史原实际结果（原时点）：actual_result = controller independent full re-run at 1d0d93d: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; checker unittest 48 passed; service 2 passed / 0 failed; workspace 95 result lines, 1263 passed / 0 failed; separate real-process Python harness 47 passed / 0 failed in each normal, optimized reverse and restored run; both semantic response mutations detected; two root probes pass on both head and parent and are boundary checks, not red-to-green defects
历史原风险与不可声明：risk = The primary cargo entry is the existing MCP service 2-case regression, not the HTTP service or the additional 47-case Python harness. The bound additional Python script and normal/optimized-reverse/restored logs contain the substantive new coverage. No real Claude, in-flight cancellation propagation, business v2 negotiation, production effect or release
历史原回滚约束：rollback = revert PR #111 through an independent revert PR; accepted-head parent e7ec4904b42e8c6f0d20e425b3bdc77c3109c180; retain evidence, costs, dispatch history and revocation state
**历史first阶段（Codex原记录L207–216；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-066 / PR #111 — 本地MCP stdio一致性harness
历史first事实：- PR #111 fixed1d0d93d1dc42deae83056bfc8602fd3bf2760f3d，merge200c43bc19368ff52b064e67eb1b14e35f096d39，at2026-10-01T16:11:27Z；accepted/merge tree53426fe775001c01f8e09437c4a3d4e4b1faf80c相等，两父basee7ec490+head精确相等。admin=false，main已ff-only。首次body PATCH EOF保留，同请求重试后全文读回通过，全部硬前置成功再merge。清理随后，QA保留；台账周期产品1。
历史first事实：- 固定1d0d93d1dc42deae83056bfc8602fd3bf2760f3d，父e7ec4904b42e8c6f0d20e425b3bdc77c3109c180；原fd4a1aa1620b73a2df1f8c567bfd0770ece2777c从6f0e5fa叠放，唯一patch cmp0。唯一scripts/test_mcp_stdio.py +636/-0，全文已读，SHA2565663ebb36a611087a08969c9d95eb58bc5ab32ec90c1a0ed430d2a62545541e9，WT与父树clean。
历史first事实：- 全新target-ctrl-ag066：ctrl-ag066-1d0d93d.log*八门exit0、1263/0=父1263+0，90bin+5doc，检查器48。额外真实disabled stdio ordinary/-O reverse/restored各47/0 exit0，各54正常EOF+7预期帧拒绝exit1，无强制退出。原实施基线1215/0；初sandbox1210/5环境失败保留，R1重跑通过。
历史first事实：- root两自写边界，冻结source-test-mcp-stdio-5663ebb.py同SHA，probe.py SHA2565bc88fb8e089b42f4e83d68d02e249d5f2298b68fed8abac43a25a7b8b07a91a。head ctrl-ag066-adversarial-head-1d0d93d.log与实际父ctrl-ag066-adversarial-base-e7ec490.log均2/0 exit0。P1真实UTF8/转义JSON、key顺序、空白/CRLF在三写工具返回同完整幂等结果且原对象不变；P2非法goal不占key，经两次EOF/restart后合法首次成功、冲突不改原对象、成功缓存前仍做业务验证。测试覆盖PR不改产品，两边均通过是边界核验，不冒称产品修复红灯。
历史first事实：- root mutate_responses.py SHA256f17d8abf18adaf76d8d29f046c709d7c254184a32d24d82c4f64e1337784a513，真实进程解码后仅改响应。精确错误码invalid_input→forbidden且双通道一致：2hits，45/2，exit1；合法text与structured verification不同：30hits，17/30，exit1；无errors，均正常断言失败；恢复47/0。初root probe把非法Unicode/斜杠放request_key导致父0/2为设置错误，原probe-invalid-key-setup.py/日志保留，修正版将Unicode放合法goal/details/content，两树一致。正文首修改本地文件尚不存在exit1未API，已纠正。
历史first事实：- verified仅E02/E16.4 §6.1L605/L609、E16.4L1443–1445、V002四工具旧v1/本地身份/重连/错误和严格帧子范围；V039只声明本机fixed rmcp3.3.0、rsia0.2.0、协议2025-06-18实测边界。精确四golden与2000字节预算，47矩阵覆盖，不更改生产/CI/锁。
历史first事实：- 不声明真实Claude/其他宿主/跨平台/业务取消/v2协商/全表零写/真实消费者/计费安全/质量收益/可发行。stderr临时文件不等于绝对磁盘上限。回滚参考父e7ec490，纯删除新脚本，保留QA/费用/撤销事实。
历史first事实：- 卡AG-066.md SHA2560e995b9ad068fc5d99f2ffab51105babd1262b0bd2f3fd94458567f1d448219c；R1环境b56f9ddefe9863edf7e3f89cfe03a52b2fd45c70bccef3ef5241dc8009a49824。产品返修0/环境裁决1，无冲突。实施push自动审批拒绝原文保留；root实际payload/remote public admin权限/直接人类指定hash授权新事实同请求复核获准，未绕过。完整report bbae556312f720436980cba6d36069615ecbdcaab52d28957269161f6a5006c7。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E02/E16.4；E16.4.ag066_local_stdio_harness [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-066.md`，文件SHA256 `0e995b9ad068fc5d99f2ffab51105babd1262b0bd2f3fd94458567f1d448219c`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag066-1d0d93d.log.test`，文件SHA256 `b076d78753ceee8f8ca09fcef3487bfe632ada15b3cdfd4a06f0265e38af1270`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag066-adversarial-head-1d0d93d.log`，文件SHA256 `8bc496ad952911ba917c96ac18853a5b8f5dc93b06fd0d9ff49b6125ecccb8b1`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag066-adversarial-base-e7ec490.log`，文件SHA256 `c88263ab0967f7ba21bd0224e6d063ee7321018fdb00f2fba8eae68d9ea7252b`。
### PR #112 — AG-067（第四轮历史）
冻结M6角色：公开head a18cedd50ce5e65bd090f0e17d7a261c3e0e1698；本地验收head a18cedd50ce5e65bd090f0e17d7a261c3e0e1698；实际merge e7ec4904b42e8c6f0d20e425b3bdc77c3109c180；mergedAt 2026-10-01T15:54:38Z。
合并父 c6e13e906ea888916294e6deac77c37f4391225f, a18cedd50ce5e65bd090f0e17d7a261c3e0e1698；tree f0c1d8c2b6cfc7a71a2c8e9c9684cf1f68faf7a9。原双父合并角色；不是本轮网络重查或重新执行。
**历史first阶段（Codex原记录L194–203；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-067 / PR #112 — 第三批台账和索引
历史first事实：- 卡cards/AG-067.md SHA256c04e4247ab74873f489552b3f8fee6a79d16bc64a4fe7ec12c309621c32abbed，无返修裁决；basec6e13e906ea888916294e6deac77c37f4391225f→heada18cedd50ce5e65bd090f0e17d7a261c3e0e1698，无叠放；mergee7ec4904b42e8c6f0d20e425b3bdc77c3109c180，at2026-10-01T15:54:38Z，双方treef0c1d8c2b6cfc7a71a2c8e9c9684cf1f68faf7a9、两父相同。
历史first事实：- 固定heada18cedd50ce5e65bd090f0e17d7a261c3e0e1698，实际父c6e13e906ea888916294e6deac77c37f4391225f，三白名单文件+277/-12全部独立读审，无叠放/冲突。fold SHA2566378304cbb298b5045f94c1552862cbf3210e6812aff5e14cc549d0778357fa4仅标题层级变化，64旧verified记录不变。
历史first事实：- fresh target-ctrl-ag067，qa/ctrl-ag067-a18cedd.log*八门exit0；1263/0=父1263+0，90测试二进制+5doc组；Python48，定向ctrl-ag067-support-scope-a18cedd.log14/0。
历史first事实：- root Python probe-ag067/probe.py SHA2564444570ada20a2176b34634d082913c04985b8714dcd281bf49454f5408b37e7；head ctrl-ag067-adversarial-head-a18cedd.log全通过，实际父ctrl-ag067-adversarial-base-c6e13e9.log因三新记录缺失exit1，旧篡改/正向/clean通过。三条source/merge第二父/tree/祖先/blob/input及全部日志hash、全量8退出码/计数/额外engine18与core4 blob逐项核对；5篡改分别旧AG042输入、新AG062输入、065跨scope日志替换、064错误merge、064新输入均拒。
历史first事实：- AG065两crate同basename，primary明确core16，engine18额外绑定；输入变更前清单*-pre-ag067.json留存。原始日志不改。初registration替换误匹配set导致查旧记录的exit0保留于setup-old-records，不作成功；纠正后真实parent缺三记录exit1。
历史first事实：- verified仅E00/E16.6：E00L1181–1189/E16.6L1459–1463/§18.5L2162–2166，V001/V071/V072/V073/V074/V080/V098三项已验子范围追溯。062不证明P/B真实性，065不证明类型/默认/真实消费者，064不证明完整合法集/修复生产链/旧报告迁移；066未验，V076完整链未验。
历史first事实：- 不声明新产品/完整E-V/历史归并/真实效果/发行，历史部分input缺失保持。回滚参考父c6e13e906ea888916294e6deac77c37f4391225f，保留证据/费用/撤销。按启动§5索引任务采用Python反例；产品返修0，主控输入适配已披露，审批拒绝无。合并采用fail-fast正文读回/head/当前父/clean，admin仅保护需要时。
历史first事实：- admin=false，无冲突，产品返修0；各硬前置通过后合并。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E00/E16.6；未入当前98单目标索引：台账/索引PR。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-067.md`，文件SHA256 `c04e4247ab74873f489552b3f8fee6a79d16bc64a4fe7ec12c309621c32abbed`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag067-a18cedd.log.test`，文件SHA256 `bc6a2bad41852e7d4107bb03b8cb06d35677721d5c57d6d4b4f7a669029b5469`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag067/probe.py`，文件SHA256 `4444570ada20a2176b34634d082913c04985b8714dcd281bf49454f5408b37e7`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag067-adversarial-head-a18cedd.log`，文件SHA256 `81abc30b69d0e2542cc5200ad154404632c31b725ef3f43b90573958c2e34890`。
### PR #113 — AG-068（第四轮历史）
冻结M6角色：公开head 75754911a01aab756d5f17b9d6fef8bb3959de33；本地验收head 75754911a01aab756d5f17b9d6fef8bb3959de33；实际merge 212c78488b403397eaebaa65a6d280b9c6be3899；mergedAt 2026-10-01T16:54:00Z。
合并父 200c43bc19368ff52b064e67eb1b14e35f096d39, 75754911a01aab756d5f17b9d6fef8bb3959de33；tree 9db6431801f1989be75792735aa7f8a7eca2b0e5。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E08/verified_subscopes/7`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E08.ag068_native_authority_cleanup；description = Registered native authority successors are cleaned in new revocation jobs
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 75754911a01aab756d5f17b9d6fef8bb3959de33；pr = https://github.com/acosmi/RSIAgent/pull/113；merged_sha = 212c78488b403397eaebaa65a6d280b9c6be3899；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/import_source_successor_cleanup_v42.rs；command = cargo test --locked --offline -p evo-engine --test import_source_successor_cleanup_v42；exit_code = 0
历史原证据登记：input_digest = d2a606dbae7cf15e2b3e7432215ac599af4fd98ca0d08d670421d944baade678；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-068-7575491.json；log_path = out/audit-20260930/qa/ctrl-ag068-7575491.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E08/E16.1; section 11 L1077, section 11.3 L1110-L1112, section 11.5 L1132-L1134 and E08 L1295/L1299; V017/V018/V075 limited logical cleanup of valid native authorities in a registered typed closure, including non-primary sources
历史原实际结果（原时点）：actual_result = controller independent full re-run at 7575491: fmt, clippy -D warnings, cargom test --workspace --locked --no-fail-fast via rsproxy (not --offline), build, both smokes, support-scope checker and unittest all exit 0; checker unittest 48 passed; import_source_successor_cleanup_v42 10 passed / 0 failed; workspace 96 result lines, 1273 passed / 0 failed; two root probes pass on head and both fail after successful compilation on the actual parent; actual old-program Failed database boundary 1 passed / 0 failed
历史原风险与不可声明：risk = Only new Pending/Running jobs. Real import and Host writers are exercised with explicitly manual dependency edges; this does not implement the full production import-learning/new-use chain or all V076. The actual old-program Failed database still remains Failed at this accepted head; AG069 local recovery is separate and unmerged. No physical erasure, arbitrary late writes, production or effect claim
历史原回滚约束：rollback = revert PR #113 through an independent revert PR; accepted-head parent 200c43bc19368ff52b064e67eb1b14e35f096d39; retain evidence, costs, dispatch history and revocation state
**历史first阶段（Codex原记录L220–232；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG-068 / PR #113 — 已登记native authority后继清理
历史first事实：- PR #113固定75754911a01aab756d5f17b9d6fef8bb3959de33，merged_sha212c78488b403397eaebaa65a6d280b9c6be3899，mergedAt2026-10-01T16:54:00Z；accepted/merged tree9db6431801f1989be75792735aa7f8a7eca2b0e5相等，两父200c43bc19368ff52b064e67eb1b14e35f096d39与验收head相等。fail-fast正文读回/head/base/clean全部成功，admin=false，无冲突，main已ff-only。清理随后，QA保留；#112后产品2。
历史first事实：- 固定head75754911a01aab756d5f17b9d6fef8bb3959de33，实际父200c43bc19368ff52b064e67eb1b14e35f096d39；原1ef06f10e2ffb1869cbf714df5c381b28963596d从c6e13e9叠放，单提交before/after-rebase.patch逐字节cmp0，literal fullref/oldhead lease成功，无冲突。2白名单文件+884/-0已全文审阅并重核SHA一致；产品47行严格wire分类，其余837行10测试，现有测试未动。
历史first事实：- fresh target-ctrl-ag068，ctrl-ag068-7575491.log*八项exit0，1273/0=父1263+10，91测试二进制+5doc，Python48。实施报告qa/impl-ag068/handoff-report.md SHA25676a4a9f6736c009075f93bb84e2cb01dba07f449e5b22540506f5a4c4d097c23，原baseline3/7、移分类3/7、schema-only8/2均成功编译后失败；27项负表schema-only25错误放行被抓。初workspace1268/5与CLI exit1环境失败保留，R1相同命令复跑1273/0/双smoke0。
历史first事实：- root冻结source-import-source-successor-pre-review.rs SHA256f36a3c5b808c68d7e74c628475bd796a4b412ebe83443227db96c37b2f0b6329；append.rs SHA256cae73ba08e84497a0719decf81195a0248a3a52c1edc68bdce07e422b5b63176。head ctrl-ag068-adversarial-head-7575491.log2/0 exit0；实际父ctrl-ag068-adversarial-base-200c43b.log0/2 exit101，正常编译且两树clean。P1双来源水位/双job交错分页共享native，只脱敏一次且original_digest不重哈；P2环与expanded后迟到native由fixpoint收齐，重复Complete无变化。手工typed边含故意绕写门明确是清理夹具，不冒称生产学习链。
历史first事实：- 额外root旧库边界old-failed-upgrade-boundary.rs SHA256c71aeb2973b285039112180950e6621f9ebde94d2415225e62814fe1d67f44b5，ctrl-ag069-old-failed-control-7575491.log1/0。实际200c43b旧程序Host+begin/cleanup生成backup DB SHA4fdb653a0f78fbefa3ef813522564a6f09e929736e95df2fe069b7ef04228b4a复制后在本head三次Admin/reopen仍Failed/pending0/三native未清；原件逐字不变。此为AG069真实升级父对照，不能算068修复旧Failed。
历史first事实：- verified仅E08/§11 L1069–1081特别L1077、§11.3L1110–1112、§11.5L1132–1134、E08L1295/L1299；V017/V018/V038/V075仅已登记闭包中新Pending/Running作业的合法native authority清理、分页重开、非primary来源与共享/环/迟到边。外层与record strict，信任/用途/identity/family/hash/excerpt绑定，diagnosis不新增Host原本没有的语义门；5outcome合法均清。未知仍Failed、直接源run删除不变，原id/digest/audit/edges/cache墓碑保留。
历史first事实：- 不声明完整V076/生产import→native接线、可信升级、所有Pending读闸、generic run gate、旧Failed恢复、任意未含新edges旧备份保护、物理擦除/完整E-V/效果/发行。旧Failed下一AG069串行，依据真源可实施，已撤销整体待合同判断。回滚参考实际父200c43b，保留水位/费用/审计/已脱敏事实，不还原明文。
历史first事实：- 卡AG-068.md c56eb468495b05a4d9e16b92d7eb3845542cda180b288f0e771a8264de851ff0，R1环境e1d2aeb90ef74cdb0b52632d6dba1d10ab75d2a51bf32bf047d57b1482ad5bff，产品返修0/环境裁决1，无冲突。子代理auto-review拒绝原文保留，root直接人类指定hash授权/actualpayload/public remote admin-push新事实同请求复核获准后建draft#113，未绕过。
历史段标题：汇总（#113后）
历史first事实：- 新增#113 212c78488b403397eaebaa65a6d280b9c6be3899；此前#98–112见各节。AG070本地导入材料与AG071UTF8隐私扫描实施，AG069旧Failed恢复下一步串行派发。056workflow权限与真正原pending保持，用户未收尾。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E03/E08/E16.1；E08.ag068_native_authority_cleanup [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-068.md`，文件SHA256 `c56eb468495b05a4d9e16b92d7eb3845542cda180b288f0e771a8264de851ff0`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag068-7575491.log.test`，文件SHA256 `ede2142e0e1feee07a49651c3ee34f307c00bef3e747ff4ee7e0899bf5c0f240`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag068-adversarial-head-7575491.log`，文件SHA256 `52555d14568efce55f6ce2389f2c9e4659e06d1448ac88277f96ed334e64a3dc`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag068-adversarial-base-200c43b.log`，文件SHA256 `fba045797c18bae737e0ab5920afff08392070d4ae0db3a9a24dcc6e0eb97e67`。
### PR #114 — AG-071（第四轮历史）
冻结M6角色：公开head 53dd17677a38abab26d31f1a57778b7fca9694e6；本地验收head 53dd17677a38abab26d31f1a57778b7fca9694e6；实际merge c0c4dbd3f6f0464c6cb492c0eb298496fd88b9de；mergedAt 2026-10-02T13:23:55Z。
合并父 212c78488b403397eaebaa65a6d280b9c6be3899, 53dd17677a38abab26d31f1a57778b7fca9694e6；tree 596687aa3d8380ee70ba4a121a90bcb42edf05bd。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E16.2/verified_subscopes/1`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E16.2.ag071_known_marker_utf8_blocking；description = Known-marker privacy findings remain blocked with UTF-8-safe byte-limited snippets
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 53dd17677a38abab26d31f1a57778b7fca9694e6；pr = https://github.com/acosmi/RSIAgent/pull/114；merged_sha = c0c4dbd3f6f0464c6cb492c0eb298496fd88b9de；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/privacy_utf8_v42.rs；command = cargo test --locked --offline -p evo-engine --test privacy_utf8_v42；exit_code = 0
历史原证据登记：input_digest = 0273ae0b27ef500358322f8e0000310a3cfb1f393c804414048b4000dda65836；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-071-53dd176.json；log_path = out/audit-20260930/qa/ctrl-ag071-53dd176.log.test；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E16.2; section11.2 L1104, E16.2 L1427/L1429, V067 L1575: known-marker blocking remains valid for UTF-8 text within original snippet byte caps.
历史原实际结果（原时点）：actual_result = ROOT independent full8 at 53dd17677a38abab26d31f1a57778b7fca9694e6: all exits 0; privacy_utf8_v42 10 passed / 0 failed; 97 workspace result groups sum to 1283 passed / 0 failed; Python checker 48 passed. Two independent ROOT public-API probes pass on head, while both fail on actual parent 212c78488b403397eaebaa65a6d280b9c6be3899 with exit101 after ordinary compilation; parent logs retain 15 real UTF-8 panics. Actual PR114 merged as c0c4dbd3f6f0464c6cb492c0eb298496fd88b9de with accepted tree596687aa3d8380ee70ba4a121a90bcb42edf05bd.
历史原风险与不可声明：risk = Only known-marker UTF-8-safe blocking at §11.2/E16.2/V067. Original 12/24/16-byte caps, marker order, matching, error policy and ASCII output remain. Scanning is incomplete and reports may contain original snippets. No absolute no-leak, broader scanner coverage, permission to export, full import-learning/V076, new V075, model/effect/deployment/release claim.
历史原回滚约束：rollback = Revert PR #114 through a reviewed independent revert; accepted-head parent 212c78488b403397eaebaa65a6d280b9c6be3899; retain immutable evidence, revocation/exposure history, costs and audit facts.
**历史first阶段（Codex原记录L233–241；原时点、原head与后来的公开/验收head分列）：**
历史段标题：未合并附录：AG071 本地验收完成（2026-10-01T17:54:07.434757+00:00）
历史first事实：- 本地验收固定head `53dd17677a38abab26d31f1a57778b7fca9694e6`，实际父 `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `596687aa3d8380ee70ba4a121a90bcb42edf05bd`。原d8a2e4769879eced02be1937882dd08216ff24f4从200c43bc19368ff52b064e67eb1b14e35f096d39叠放，单提交before/after-rebase.patch逐字节cmp0；无冲突、WT clean。尚无远端branch/PR，因此未执行force-with-lease、ready或merge。
历史first事实：- 两白名单文件+688/-6完整预审，交付与冻结SHA一致。产品仅private UTF8 endpoint helper和3调用(+12/-6)；new privacy_utf8_v42 676行10测试。ASCII窗口12/24/16、marker/顺序/等级/错误/时间policy不变。
历史first事实：- 全新target-ctrl-ag071独立八门全0，ctrl-ag071-53dd176.log*；逐行独立求和97结果行=92测试二进制+5doc组，1283/0=实际父1273+10，Python48。CARGO_BUILD_JOBS=2仅限制构建并发，所有要求命令/断言保持。root localhost完整全量/双smoke实际通过。
历史first事实：- 两root自写反例：frozen source e4c6a606f217a36c29f8f800c8e5e7024ddfac2b22e922c7224187d62d07c4f7，append b136b3ba853541286710b15713b23cb1f77785351820bed1c02d38dea95a3b05；head ctrl-ag071-adversarial-head-53dd176.log 2/0 exit0；实际父 ctrl-ag071-adversarial-base-212c784.log 0/2 exit101，双方成功编译且WT clean。P1真实export metadata name/dependency.version_req/nonprimary source-id共9输入JSON往返，P2foreign approval/evaluation notes共6输入manifest往返；旧树15实际UTF8 panic，head均精确privacy Error，入参字节不变、Clean正例仍通过。最初root ForeignMetadata::default编译设置错误保留invalid-default-setup，不算产品红灯。
历史first事实：- 实施报告qa/impl-ag071/implementation-report.md SHA256406e5e9b5ddec53e1e0054311fbfbd0038f252fed9be292b2da83e0f14fcab73；最终baseline2/8、504边界184panic、固定10/0、38ASCII findings逐字段快照、两变异101与cmp0已核。首新test误把Windows用户名目录前缀双命中当单命中只修新test且重跑原product baseline；原错误保留。初workspace1268/5明确HTTP EPERM，R1同命令1273/0；CLI初1仅readiness超时不伪称stderr EPERM，提权同命令0。
历史first事实：- 本地verified子范围仅§11.2L1104/E16.2L1427,L1429/V067已知隐私模式的UTF8安全阻断。不声明更广扫描覆盖、绝对无泄漏、授权出站、完整导入学习链/V076、V075新验证、真实模型/效果/发行。privacy报告本身可能含原snippet，政策未改。回滚参考父212c78488b403397eaebaa65a6d280b9c6be3899，不还原已脱敏内容、费用、审计或撤销事实。
历史first事实：- 产品返修0、R1环境裁决1，卡e20c8b42c67a88fbc9072491947da2b82396b9e3a445b288d89cdde5512c56a9/R1 f36826c8159b9cd97e248a46c5eb704de3e54eb3188389296addd328350adcf7。child与root两次原精确push均在创建进程前被自动审批拒绝，未外发、未绕通道；root新事实复核仍要求具体payload公共目的地授权，待用户明确。无PR/未合并/不可填写merged_sha或纳入已合并汇总；保留完整本地结果可复审。
**历史first阶段（Codex原记录L597–603；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG071 / PR114 / E16.2 / V067 merged 2026-10-02T13:27:21.555722+00:00
历史first事实：- Card cards/AG-071.md SHAe20c8b42c67a88fbc9072491947da2b82396b9e3a445b288d89cdde5512c56a9; environmentR1 SHAf36826c8159b9cd97e248a46c5eb704de3e54eb3188389296addd328350adcf7. Original200c43bc→d8a2e476→exacthead53dd17677a38abab26d31f1a57778b7fca9694e6, parent212c78488b403397eaebaa65a6d280b9c6be3899; rebasepatchcmp0/no conflicts. merged_sha=c0c4dbd3f6f0464c6cb492c0eb298496fd88b9de, mergedAt2026-10-02T13:23:55Z, head/mergedtree=596687aa3d8380ee70ba4a121a90bcb42edf05bd exactequal.
历史first事实：- Root independentfresh8 qa/ctrl-ag071-53dd176.log*, all0; Rust1283/0/97groups=92bins+5doc, Python48. Frozenroot probes head2/0 exit0 / actualparent0/2 exit101, qa/ctrl-ag071-adversarial-head-53dd176.log and qa/ctrl-ag071-adversarial-base-212c784.log; successfulcompiled both, originalinvalid ForeignMetadata default setup excluded/preserved. Current676line test plus productdiff fullyread, rawhash preflightcd216276; no newruntimeclaimed.
历史first事实：- Verified §11.2L1104/E16.2L1427,L1429/V067 knownprivacy UTF8-safe rejection only. Limits: marker/policy/snippet disclosure unchanged; no broadercoverage/absoluteprivacy/outboundauthorization/fullV076/models/effect/release. Productrepair0/environmentR1 once. Humanadmin publicauthorization afterspecifictwofiles+publicdestination explanation46033a0b; priorchild/rootapprovalrejection retained. PRfirstEOF/absentreadback/retry114 preserved. --admin used onlyexactacceptedhead; noActions/release.
历史first事实：- Native ready6d8ca5/merge0e5d1c/readbackeca0c2/fetch303cd5 all0; firstprimaryff c58962sandbox1 preserved then2d6c51ff0/clean. ExactmergeQA qa/takeover-20261002/ag071-admin-merge-tree-verification-20261002.json SHA499635ad14796c8ef52ecb111a519c0042d94f91514c7d3804dad2797a406282; cleanup071pending. Rollbackparent212c, no evidence/cost/revocation erasure.
**历史first阶段（Codex原记录L613–617；原时点、原head与后来的公开/验收head分列）：**
历史段标题：r4 AG071 explicit remote deletion authorized and completed 2026-10-02T13:45:49.492867+00:00
历史first事实：- Human admin批准 directly answered exact remote branch deletion question after risk disclosure. FreshPR1143a92d0 MERGED/head53dd176/mergec0c4dbd verified. Native08df0d exit0 actuallydeleted only wrokbot/ag-071-privacy-utf8-snippets. Original two automatic rejection records preserved; no bypass. QA qa/takeover-20261002/ag071-remote-branch-cleanup-authorized-complete-20261002.json SHAce27bca1cdeb970d8304d762c07338d735029c0deef480bf8ea5b3802b3a6e3d. AG071 local WT/branch alreadyremoved, rawQA preserved.
历史first事实：- Goal tool currently actual active (7e1e1c), not fabricatedresumption. Actual gpt-6.1-sol/max SC spawn now succeeds; old Unknownmodel historicalrecord retained, modelavailability blocker now resolved. Rootactualmodel not exposed, no modelidentity fabricated. Continue AG072 restack/fresh8/probes before exacthead merge.
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E16.2；E16.2.ag071_known_marker_utf8_blocking [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-071.md`，文件SHA256 `e20c8b42c67a88fbc9072491947da2b82396b9e3a445b288d89cdde5512c56a9`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag071-53dd176.log.test`，文件SHA256 `2de47abb17527ce72ab2deb204d73d5a73998e570d5e293cdb37b6c72f765b30`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag071-adversarial-head-53dd176.log`，文件SHA256 `263c444b2aa1444b590accb3f10b383e060e6f8beabebc9affda08faa3bde61c`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag071-adversarial-base-212c784.log`，文件SHA256 `b77a62253bb8e4ca1dc5183b632a0cbc48c73b842ed81590d7f5ca655ac69993`。
### PR #115 — AG-072（第四轮历史）
冻结M6角色：公开head f689d2f28f7d9064fcd969def0808ac32070917e；本地验收head f689d2f28f7d9064fcd969def0808ac32070917e；实际merge 9b9084a328b78d7376247b78cc26e75235a09afa；mergedAt 2026-10-02T14:01:05Z。
合并父 c0c4dbd3f6f0464c6cb492c0eb298496fd88b9de, f689d2f28f7d9064fcd969def0808ac32070917e；tree edcc054cf4af3c393dce3b58a95af88ee167f0e0。原双父合并角色；不是本轮网络重查或重新执行。
**历史first阶段（Codex原记录L466–471；原时点、原head与后来的公开/验收head分列）：**
历史段标题：2026-10-01T22:26:02.543764+00:00 AG072具体台账外发自动审批拒绝
历史first事实：- 精确90cef832e0716011010183f0b036f7317f02aee2三白名单向public acosmi/RSIAgent分支的首个push在执行前被auto-review拒绝，原文qa/ag072-push-approval-rejection.txt SHA214c4dc9f3856ecc68925ce44a3ad0715f422a2ab34eb9c64bafe5d0433e85bc；理由要求直接人类具体内容/公共目的地授权。与此前AG071产品拒绝分开；当前AG072自身现有一次拒绝。
历史first事实：- 已async明确三文件、+830/-14、固定head、公共目的地、会公开实施状态及证据路径摘要，尚待人类回答；不重试/换通道。无PR/未合并/不增加main/合并计数。本地全量1273/0、scope14、Python48、两父缺登记与head五篡改证据、可审PR正文保留。继续其它本地任务。
历史first事实：AG072有限验收：E00/E16.6登记一致性，卡fdef1d93b3f584e15cf4136e5a8f765000ae147ff8badf8e52cbe472f1aa90ff、R1 2b42361037c441d0f980d645d535cf4b3e9e33731b57381016503e8a3f81dba5；父212c78488b403397eaebaa65a6d280b9c6be3899→固定head90cef832e0716011010183f0b036f7317f02aee2，tree1c790c05971d2eb31b0e092fb3976fca3b1f2df8。無叠放/冲突、admin未用、无merged_sha。全量qa/ctrl-ag072-90cef83.log*八门0、96组91bin5doc、1273/0；专项14/0。probe-ag072/probe.py SHA4ce49e8cc4212b6663063411da4c75251cbc6e7fb46880c6540c8d501dab8650；实际父两新增缺失FAIL，head登记/源git/blob/input/log/count核验及五篡改全过，两树clean。root证据ag072-root-evidence.json SHA2749ea65e83a856685bbbbc0d9af81e4641505ba9e4764665d5fbf6bdef9ff21。739行事件fold仅升标题，67旧verified对象不变。公开部分不含原始QA或真源正文，不代表完整E/V能力；较原流程偏离是发布被拒，保留本地待授权。main若推进须重新叠放与全量验收。
**历史first阶段（Codex原记录L618–624；原时点、原head与后来的公开/验收head分列）：**
历史段标题：r4 AG072 / PR115 actually admin merged 2026-10-02T14:02:46.634137+00:00
历史first事实：- PR115 https://github.com/acosmi/RSIAgent/pull/115 MERGED at2026-10-02T14:01:05Z, fixedheadf689d2f28f7d9064fcd969def0808ac32070917e, parentc0c4dbd3f6f0464c6cb492c0eb298496fd88b9de, merged9b9084a328b78d7376247b78cc26e75235a09afa. Native ready8f249c/finalpreflight64ec84/adminmerge02e05a/view248cd0/fetchb60ec8 all0. Both trees edcc054cf4af3c393dce3b58a95af88ee167f0e0 equal; actual two parents correct. Primaryff actual0 main=origin9b9084a/clean. QA qa/takeover-20261002/ag072-pr115-admin-merge-tree-verification-20261002.json SHAc4b0b40c27ed878d47dbdd8b95135906f9b07176ca5c6d84f1bed7b60092d2c9.
历史first事实：- OriginalAG072cardfdef1d93; R1e67760bd; R2concurrency7ef5f2b0. Old90cef/parent212c rebase3e16f4→f689/parentc0, exactpatchb5637941, no conflicts. Rootfresh8 all0/1283-0/97groups92bin5doc/Python48/scope14. Newhead5tamper allreject/positivepass/cleanexit0, actualC0parentmissing066068 eachFAIL/exit1, oldcontrolsPASS. AcceptanceQA8917a069/log8db86b15/testdf4e4522/probe3ed32765/helper7de07a14; rootpreflightstrictglobalheld twice0487c0/91a1d5 beforechild, R2correctedwithin3routes with foreignCeabCode untouched. All historicalfailures/oldgrant/QA preserved; publicapproval46033a0b and --admin=true.
历史first事实：- Verified E00/E16.6 consistency only, scope clauses1185/1459–63/2150–66/2190–94, V001/V071/V072/V073/V074/V080/V098 limitedregistration. No fullE/V/history126/V076/model/paid/effect/release. Rollbackparentc0 via reviewedrevertPR; preserveevidence/cost/revoke. 18r4actualmerges #98–115. Next safe072cleanup andrefresh082/085/083 against9b9084a, no oldparent fullproofreuse.
历史first事实：- User updated thinking depth: new dispatches inherit latest actual parent setting; exact UI effort enum unavailable here and not invented. Prior completed read-only agents actuallyran gpt-6.1-sol/max and remain historical; no silent alteration of recorded past depth. Root/model identity likewise not invented.
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E00/E08/E16.4/E16.6；未入当前98单目标索引：台账/索引PR。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-072.md`，文件SHA256 `fdef1d93b3f584e15cf4136e5a8f765000ae147ff8badf8e52cbe472f1aa90ff`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag072-90cef83.log.test`，文件SHA256 `17cf21dbcec969078c15f29a260898b905c05ec3f99864c3cfb531b724f91fb1`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag072/probe.py`，文件SHA256 `4ce49e8cc4212b6663063411da4c75251cbc6e7fb46880c6540c8d501dab8650`。
### PR #116 — AG-083（第四轮历史）
冻结M6角色：公开head 215811ff8848817e72f0e3226b81d57e0fef35ea；本地验收head 215811ff8848817e72f0e3226b81d57e0fef35ea；实际merge 7fa290883136f30fcdc87ff5a4de293aa0714507；mergedAt 2026-10-03T10:16:16Z。
合并父 9b9084a328b78d7376247b78cc26e75235a09afa, 215811ff8848817e72f0e3226b81d57e0fef35ea；tree f4cf6018329b46c0633ba3a8a6ccd374bd6b6e94。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E05/verified_subscopes/2`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E05.ag083_partition_admission_rejection_atomicity；description = Rejected in-memory partition admissions add no partial raw, normalized or family registrations
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 215811ff8848817e72f0e3226b81d57e0fef35ea；pr = https://github.com/acosmi/RSIAgent/pull/116；merged_sha = 7fa290883136f30fcdc87ff5a4de293aa0714507；pr_state = merged
历史原测试登记：test_entry = crates/evo-core/tests/partition_admission_atomic_v42.rs；command = cargo test --locked --offline -p evo-core --test partition_admission_atomic_v42；exit_code = 0
历史原证据登记：input_digest = dffe2f91c22a4f48731a0b387e2950c517612031cad76b9bc1b14e0b0fdf4417；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-083-215811f.json；log_path = 本地scratch原全workspace记录（full-workspace-MAIN-original-native.json；not_located；完整原值见上述原对象/log_path）；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E05; section3.2 L265-L269 and3.2.2 L292/L294, E05 L1253/L1257, V011 L1511/V031 L1531/V084.c,d L1641-L1643: pure in-memory single-admit rejection atomicity with existing purpose/exposure contracts preserved.
历史原实际结果（原时点）：actual_result = ROOT final tree f4cf6018329b46c0633ba3a8a6ccd374bd6b6e94: full8 exits all0; partition_admission_atomic_v42 31 passed / 0 failed; 98 workspace result groups sum to 1314 passed / 0 failed; Python48 passed. Two independently frozen retry probes each pass on current source and each fail on actual parent9b9084a328b78d7376247b78cc26e75235a09afa with exit101 after both ordinary no-run controls exit0. Failures assert unconsumed raw/family state. Fixed accepted HEAD215811ff8848817e72f0e3226b81d57e0fef35ea merged in PR116 as7fa290883136f30fcdc87ff5a4de293aa0714507 with identical accepted tree.
历史原风险与不可声明：risk = Pure in-memory PartitionRegistry::admit rejection consistency only; preserve successful registrations, sticky exposure, first-error priority and existing field contracts. Rejected calls no longer leave partial new raw/normalized/family members. No persistence or production wiring, source authenticity, full contamination/epoch machinery, migration of old partial state, full V011/V031/V084/E05, benefit/approval/release claim.
历史原回滚约束：rollback = Revert PR #116 through a reviewed independent revert; accepted-head parent 9b9084a328b78d7376247b78cc26e75235a09afa; retain immutable evidence, revocation/exposure history, costs and audit facts.
**历史first阶段（Codex原记录L637–646；原时点、原head与后来的公开/验收head分列）：**
历史段标题：CTRL-AG083 / PR116 actual merged 2026-10-03T10:16:16Z
历史first事实：- E05有限纯内存拒绝登记原子性；条款§3.2L265–269/§3.2.2L292,294/E05L1253,1257/V011L1511/V031L1531/V084.c,dL1641–1643。cards/AG-083.md SHA5f3b122f3ac945e3e81bccaf9de15e0b05c3939f314f5487a1d4743b3fd6ad23、所有返修/静态/runtime/card记录保持；公共授权卡3aebd429…。原212基线历史不改，当前parent9b→acceptedHEAD215811ff8848817e72f0e3226b81d57e0fef35ea/treef4cf6018329b46c0633ba3a8a6ccd374bd6b6e94。最小let-chain clippy修复后source53108Babe070…，原53128Ba819…backup、31test不变。
历史first事实：- ROOT八门真0，1314Rust/0=parent1283+31，Python48/0。完整8QA/MAIN位于 out/audit-20260930/qa/takeover-20261002/ag083-new-source-full8-and-two-independent-counterexamples-ROOT-local-acceptance-r1-20261003.json，12844B685fb6c4…。两独立fixture3342B06c204d8…，current/parent分别ordinaryno-run0；current各1/0/1filtered/exit0，parent各0/1/1filtered/真实101，原semanticassert失败、非编译错。14MAIN全wire/现场guard0，原owner15directional/golden/mutant原scope保留。
历史first事实：- localexact2filescommit/source206/oldindex848dbackup、SC1352B47b86e…及publicpatch33504Bde41d87c…均同验收源；merged_sha7fa290883136f30fcdc87ff5a4de293aa0714507，merged/head/origin treeexactf4cf6018329b46c0633ba3a8a6ccd374bd6b6e94。main实际ff-only0clean。match-head-commit固定，admin_flagfalse（CLEAN无requiredreview）。ROOTmergeQA out/audit-20260930/qa/takeover-20261002/ag083-fixed-head-public-PR-ROOT-r1-20261003/ROOT-actual-fixed-HEAD-merge-and-main-tree-review.json SHA48d915eb0d133aef25347e7c3fde45eda220ca6fa5bac51f12cc3840c68730b2；originalnative out/audit-20260930/qa/takeover-20261002/ag083-fixed-head-public-PR-ROOT-r1-20261003/original-native-ready-merge-fetch-tree-and-main-ff.json SHA89074a70cc8450498cb74c0519b280e08418dd657d3ae3b24f3b235f076065db。Codex真实coauthor，actualmodel/depthnot_exposed。ROOTdirectnative替代旧wrapper，resource0/query修正及旧failed/truncated历史全部保留。
历史first事实：- 不声明持久化/生产/来源真实/完整E05/V011/V031/V084/隐藏题独立/benefit/release。failed-call部分写入有意取消，旧success/exposure/errorpriority保留。回滚需对parent9b审revertPR保留成本/QA。cleanup三cache候选审完未删除；19actualr4merges98–116，完整goalACTIVE，不收尾。
历史first事实：AG083 PR116 actualcleanupdate 2026-10-03T10:45:15.947371+00:00: 三stoppedtargets33163981/41474392/41474393已retired、WT normalremove/branch-d/仅083remote exact215811ff lease-delete全部真实0；postreview89f6c0/f74844均absent。RAW/pre/post/metadata及sourcebackup/QA全留，ROOT原checkpoint out/audit-20260930/qa/takeover-20261002/ag083-cleanup-and-ag085-newcurrent-restack-ROOT-original-checkpoint-20261003.json SHA921649018907df594d6b57aa833f2cf3d39194e378581d03e362a151cdb3ef6f。旧072拒绝分支未动。AG085当前207已restack7fa，旧scope不当current。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E05；E05.ag083_partition_admission_rejection_atomicity [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-083.md`，文件SHA256 `5f3b122f3ac945e3e81bccaf9de15e0b05c3939f314f5487a1d4743b3fd6ad23`。
- 原全量日志/运行原件导航：not_located；保原first胶囊与原证据，不猜文件。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag083-new-source-full8-and-two-independent-counterexamples-ROOT-local-acceptance-r1-20261003.json`，文件SHA256 `685fb6c4c10b2494762b0daf771213dcdd81efde48023af88ec7d644da3e5fb1`。
### PR #117 — AG-085（第四轮历史）
冻结M6角色：公开head 9f7038d1d033de500d858b188bbf344086821b1d；本地验收head 9f7038d1d033de500d858b188bbf344086821b1d；实际merge ddde63c6ab98a2528258d265999bb902a08b0f55；mergedAt 2026-10-03T11:47:56Z。
合并父 7fa290883136f30fcdc87ff5a4de293aa0714507, 9f7038d1d033de500d858b188bbf344086821b1d；tree 10725a0c6a86397adfbd415bd81581f399626554。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E16.2/verified_subscopes/2`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E16.2.ag085_utf8_privacy_boundary_regressions；description = UTF-8 privacy boundary and public-API refusal regressions for the unchanged parent scanner
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 9f7038d1d033de500d858b188bbf344086821b1d；pr = https://github.com/acosmi/RSIAgent/pull/117；merged_sha = ddde63c6ab98a2528258d265999bb902a08b0f55；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/privacy_utf8_boundary_v42.rs；command = cargo test --locked --offline -p evo-engine --test privacy_utf8_boundary_v42；exit_code = 0
历史原证据登记：input_digest = 6cc428709baa5da6aa5dfe99c0aba02576ec8ec51d0ce4f4b8c435091916364c；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-085-9f7038d.json；log_path = out/audit-20260930/qa/takeover-20261002/ag085-current7fa-full8-direct-ROOT-run-r1-20261003/full-workspace-test-MAIN-original-native.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E16.2; section11.2 L1104, E16.2 L1427/L1429, V067 L1575: 12 UTF-8 boundary, original byte/digest identity and privacy refusal regressions; actual parent scanner unchanged.
历史原实际结果（原时点）：actual_result = ROOT fresh full8 at exact rebased HEAD9f7038d1d033de500d858b188bbf344086821b1d: all exits0; privacy_utf8_boundary_v42 12 passed / 0 failed; 99 workspace result groups sum to1326 passed / 0 failed; Python48 passed. Two public-API probes each pass on both head and actual parent7fa290883136f30fcdc87ff5a4de293aa0714507. Four separate source copies compile normally with exit0; boundary-retreat and metadata-name-gate mutants each produce semantic exit101 with0 passed /1 failed /1 filtered. Scanner is byte-identical to parent containing AG071. Actual PR117 merged asddde63c6ab98a2528258d265999bb902a08b0f55 with accepted tree10725a0c6a86397adfbd415bd81581f399626554.
历史原风险与不可声明：risk = Test-only 12-test UTF-8 boundary/privacy rejection regression subset at §11.2/E16.2/V067. Scanner equals actual parent containing AG071. Both parent independent probes are positive, and semantic red cases are isolated mutants after ordinary compile0. No new scanner policy, redaction/authorization implementation, broad privacy route, absence of leaks, real host/model/effect/deployment/release claim. Optional golden writer remains explicitly selected local allowlist, not exercised in full8.
历史原回滚约束：rollback = Revert PR #117 through a reviewed independent revert; accepted-head parent 7fa290883136f30fcdc87ff5a4de293aa0714507; retain immutable evidence, revocation/exposure history, costs and audit facts.
**历史first阶段（Codex原记录L647–665；原时点、原head与后来的公开/验收head分列）：**
历史段标题：2026-10-03T11:12:50.038333+00:00 ROOT AG085 当前六门真通过，第七门原生失败已定位，继续窄修
历史first事实：当前HEAD9f7038d1d033de500d858b188bbf344086821b1d/parent7fa/tree10725a0c6a86397adfbd415bd81581f399626554保持。fmt7fbd3c54、Clippy51798c35、workspace e5d26d1e(1326Rust/0)、builda1189bc3、smoke-workspaceae77fc99、smoke-clid7bccd82均MAIN真实0且pre/active/post guard全true0，不重跑已验收的六门。第7 full-support-scope-check native722b25真exit1，源码/资源guard仍true0，原MAIN2874B483dfba6与拒绝QA2078B9e3625a6原样保留。错误是plan把 --manifest 传绝对路径；checker safe_relative/inspect_relative明确只接仓库相对路径。Python48未执行，完整八门/两独立探针/PR接受均null。
历史first事实：ROOT新两张窄STATIC卡已实际c79d98创建：最终两门overlay2877B29addbe1，只改gate7 manifest为reports/support-scope.json、gate8不变；probe单行externalR2卡2317B17f9d538，仅own_manifest成员比较由STATIC.iterdir改为SOURCE_STATIC.iterdir。原probeR1 SC1536B59f51f96拒绝保持；其余857文件/177目录/4copies/4targets不重写。075/083各自产生外部闭包，SC与ROOT独立物理审后才ROOT发fresh runtime/grant并直接执行native，禁止把静态变绿当实际通过。当前31.1279GiB来自真实df0a65b6 Available32640000KiB=33423360000B，空间非当前阻塞；main70929f clean。ROOTcheckpoint 311171B SHA3020c1bb385cf361b8b3ae2199193ec803083ddd4025014463ae8edacd79d717。
历史first事实：goal ACTIVE、30分钟heartbeat rsiagent-30 已设且继续，不冒称已跑过；ROOT+3代理在授权边界内持续推进，其他在途队列按current-main规则保留，082/072/056公共阻塞各自独立不重试绕过。所有源/index/未提交修改/历史QA和原失败保护。早先两个wrong filename只读失败和大清单输出截断为诊断，不作接受依据。
历史段标题：2026-10-03T11:52:00.391776+00:00 ROOT AG085完整验收与PR117真正合并，持续完整目标
历史first事实：当前唯一新增privacy_utf8_boundary_v42.rs17432B83edbdb3…保持原12测试，scanner已由AG071修复，实际当前source207。完整八门真实0：原R1前6/1326Rust，R2相对manifest28b41ab9…、Python48 ddede73d…；第7旧绝对manifest真722b25 exit1保留，R1 probeSC59f51拒绝保留。独立两probe的HEAD和parent均正例，因为父有修复；4种208/207/208/208副本正常no-run0，四阳性各1passed0failed1filtered/exit0；boundary原charboundarypanic及metadata原name privacy matches断言各真实101/0passed1failed1filtered，非编译失败。ROOT独立全18原native/counts/guard/session链和最终具体testname核过，taskQA20269B8062a4c5b9b0087c9034cc1d86bd0bb03d37d4e55d04b20d48e8bc8761d07e0c。有限SC fixed2094B3eec5b2e…及R2observer1631B491dc683…/SC1164B05e18fe9…，STATIC不冒称runtime。
历史first事实：ROOT只准已验固定HEAD9f7038d1d033de500d858b188bbf344086821b1d/parent7fa/treefrozen10725a0c6a86397adfbd415bd81581f399626554。实际draft PR117 https://github.com/acosmi/RSIAgent/pull/117，公开head/base/body与唯一463行新增byte核QA1496Bb80b09f7…通过；body1945Bab9d17a0…只finitebehavior/实际acceptance，私有QA/方案/工件没入提交，两optional golden目录字面仍在test，不伪称无绝对路径。完整fullindexpatch18217B2c10c302…同旧四patch；最初短index18151B53d1b0f7…仅格式差异保留，不作fullindexequal。ready和--match-head9f合并均实际0；CLEAN/无需review故未--admin。actual merged_shaddde63c6ab98a2528258d265999bb902a08b0f55，mergedAt2026-10-03T11:47:56Z；merged/head/origin三tree均10725，实际parents7fa+9f、HEAD祖先0。旧main206source/index完整209文件closed177765B95b45918…在ff前保护；本地ff-only真实0/currentmainorigin ddde clean。ROOTmergeQA 1958B SHA1802e4c142f9fdcef7ca746e9385e0a6ef84cf2bd1101366bcb2047fad32d602，原public/native与ROOT准备错误真实保存 410339B SHA5394c1896a03264f078cff36bed236eb98a9b91f0be01cf5f3e04c00ebb26e22。累计20项r4 PR98–117已真正合并。
历史first事实：可声明仅§11.2/E16.2/V067已知marker合法UTF8snippet及既有privacy拒绝publicAPI回归有限子范围，不闭完整隐私/V067、无泄漏、生产、host、付费、效果、发行。rollback经独立审revert保留历史。ROOT脚本初input转义/container/短indexhash等验证错误发生于接受文件写入前；最后d00b31真正独立PASS，不改任何测试/原工具结果凑通过。
历史first事实：30分钟状态实核2026-10-03T11:49:51Z goalACTIVE，main/origin ddde clean，dfa2d5 disk29120000KiB=29818880000B，第一ps sandbox127失败不冒健康；另d5adc1 permitted真实0/compiler空。Heartbeat rsiagent-30原设置继续，未冒称scheduler已跑。085五currenttargets均MAIN已终止，SC纯FS候选audit在途，ROOT尚待freshconsumer/metadata证据后delete/cleanWTbranch。下一个ROOT台账/index六小时增量，公开69旧记录机械保持，补实际071/#114、083/#116与085/#117子范围；AG084未授权实施不占号。069等待下一个实际安全main epoch再叠放/fresh8，不复用旧9b。082/072/056各独立public阻塞不绕过，其余全目标继续。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E16.2；E16.2.ag085_utf8_privacy_boundary_regressions [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-085-accepted-fixed-HEAD-public-PR-and-admin-merge-R1-20261003.md`，文件SHA256 `fa470a35b2dcabd12e56798bc932197824055e3520d9992f58d02f487e73bcd2`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag085-current7fa-full8-direct-ROOT-run-r1-20261003/full-workspace-test-MAIN-original-native.json`，文件SHA256 `6e6c4b9e092bed3fca179f605cd79d02b4fa0a39292a1730960e963c73811c42`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag085-current7fa-independent-probes-direct-ROOT-run-r1-20261003/metadata-metadata-probe-MAIN-original-native.json`，文件SHA256 `2e37d9f6ba3f454dd47a96512004e41a739a5d8e0702056e6f7bd41e65ad1122`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag085-current7fa-independent-probes-direct-ROOT-run-r1-20261003/boundary-boundary-probe-MAIN-original-native.json`，文件SHA256 `acbe4473d1f5930e39906f57c452fc33c850e35e0adc239fb8a543f9c254f09f`。
### PR #118 — AG-069（第四轮历史）
冻结M6角色：公开head 9edf57e896a1b13fc7f57cf5a6392657a23da9a9；本地验收head 9edf57e896a1b13fc7f57cf5a6392657a23da9a9；实际merge 5ee01d1e2ebb2401fe94530ff286801239940259；mergedAt 2026-10-03T15:49:10Z。
合并父 ddde63c6ab98a2528258d265999bb902a08b0f55, 9edf57e896a1b13fc7f57cf5a6392657a23da9a9；tree 42de782cd2fcc6745c9f88acff9d2adba71c35d0。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E08/verified_subscopes/8`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E08.ag069_local_scoped_acceptance；description = Durable Failed-cleanup full-frontier Admin retry and Worker continuation
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 9edf57e896a1b13fc7f57cf5a6392657a23da9a9；pr = https://github.com/acosmi/RSIAgent/pull/118；merged_sha = 5ee01d1e2ebb2401fe94530ff286801239940259；pr_state = merged
历史原测试登记：test_entry = crates/evo-storage/tests/failed_cleanup_resume_v42.rs；command = cargo test --locked --offline -p evo-storage --test failed_cleanup_resume_v42；exit_code = 0
历史原证据登记：input_digest = 8fae26aea4fbd9038fc19f6e282d8602dc860be8678210af7bd55d9c3b9f83eb；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-069-9edf57e.json；log_path = out/audit-20260930/qa/takeover-20261002/ag069-current-ddde-9edf-ROOT-task-acceptance.actual-20261003.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E08; Durable Failed-cleanup full-frontier Admin retry and Worker continuation software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 9edf57e896a1b13fc7f57cf5a6392657a23da9a9; actual merge tree equals accepted tree 42de782cd2fcc6745c9f88acff9d2adba71c35d0. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #118; retain source, costs, revocation history and original QA. Accepted parent ddde63c6ab98a2528258d265999bb902a08b0f55.
**历史first阶段（Codex原记录L267–309；原时点、原head与后来的公开/验收head分列）：**
历史段标题：未合并附录：AG069 本地独立验收完成
历史段标题：CTRL-AG069-R1：旧 Failed 作业显式 Admin 续清，本地独立验收通过
历史first事实：固定 head `52de2d76b47793e9212ec81568d5d959061934d3`，实际父/当前 main `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `8c8fcd88c522b784191901cd55682ead083ec7fd`。唯一提交、无需重基，6 个白名单文件 +1923/-27 已逐文件完整阅读并核对最终 hash，交付及探针后 clean。未尝试 push、无 PR、无 merge；不可登记 merged_sha，不计已合并产品数。
历史first事实：产品在现有 cleanup_step 内仅允许 Admin 对 Failed 显式启动完整恢复轮；Worker 不新开轮但可续 Running。保留原身份/水位/创建时间及所有费用/历史，原 processed/error 入 retry 审计后重算本轮进度，重置完整 frontier 的 expanded 和两个扫描 cursor。只有真正 Complete 同事务清 last_error；仍未知则再次 Failed。旧 export 副作用在 absent/redacted 早退前按同 namespace 和 typed node 的精确旧错误事实重新检查，跨 job 共享记录也有效；不只查最后错误、不读用户任意路径、不一次载入历史，不改未知 wire 分类。
历史first事实：主控全新 `target-ctrl-ag069` 独立复跑，`qa/ctrl-ag069-52de2d7.log*` 八项退出码全部 0：fmt、clippy、workspace、build rsia、两组 smoke、support checker、Python unittest。逐行求和 98 结果组 = 93 测试二进制 + 5 doc 组，**1290 通过 / 0 失败 = 父 1273 + 新 17**（storage 15、engine 2）；Python 48。默认全量里的可选旧 DB 生成入口不等于真实旧程序升级证明，独立证据如下。
历史段标题：主控两项冻结反例
历史first事实：共同源码 `probe-ag069/source-native-cleanup-f36a3c5b.rs` SHA256 `f36a3c5b808c68d7e74c628475bd796a4b412ebe83443227db96c37b2f0b6329`，append SHA256 `c5b72f7755ea3e6172856c6d3ae2d9c69aac904c77c7622c2c876c1a46634159`。两树正常编译、同一源码/append：head `ctrl-ag069-adversarial-head-52de2d7.log` **2/0 exit 0**；实际父 `ctrl-ag069-adversarial-base-212c784.log` **0/2 exit 101**。父均因 Admin 仍 Failed 不能启动恢复而失败，非编译错误。运行后两树 clean。
历史first事实：- P1：真实 200c 旧程序生成的 Failed DB，在另一作业已抬高 namespace 水位后，Admin 一次启动旧作业，随后 Worker 每页重开 Store 续清到 Complete。原 job/source/watermark/digest/history 保留，另一个作业保持 Pending，已脱敏对象不重哈原始摘要。
历史first事实：- P2：旧恢复轮内，在已 expanded 父之后插入两个实际 Host writer 创建后故意损坏的迟到 authority 夹具。新未知使作业再 Failed；只修最后错误仍不能 Complete，分别全部修复再 Admin 重试才完成；旧失败和 retry 事件不消失。这些人工 typed 边/损坏是清理反例，不能冒称生产学习链。
历史first事实：两项使用原始 `qa/old-failed-ag069/rsia.sqlite3` 的临时副本；原件 SHA256 始终 `4fdb653a0f78fbefa3ef813522564a6f09e929736e95df2fe069b7ef04228b4a`。该库由主控实际旧树 `200c43bc19368ff52b064e67eb1b14e35f096d39` 真实 Host 写入、begin_revoke/cleanup、关闭/一致性备份生成，无 SQL 伪造 Failed；初始 processed8/pending0，三个 native 原文尚未清。
历史段标题：补充真实旧程序产物升级
历史first事实：主控另在实际 200c 旧树运行实施方冻结源码 `probe-ag069/source-upgrade-generator-1741390f.rs`（SHA256 `1741390fe71465747b3ba7471a2fb8205b3fe8bbc1da366af72871474cf41c16`），`ctrl-ag069-old-generator-200c43b.log` **1/0 exit 0**，得到独立备份 `old-failed-ag069-agent-generator/rsia.sqlite3` SHA256 `58902f602dba9e1a51dc1f5468092142bc919a5a03b1854827d51046aee4668f`。
历史first事实：同一备份副本/同冻结测试：实际父 `ctrl-ag069-new-old-db-parent-212c784.log` 正常编译后 **0/1 exit 101**（Failed != Running），head `ctrl-ag069-new-old-db-head-52de2d7.log` **1/0 exit 0**。两份日志明确 actual_old_program=true；head 从 Failed/processed6/pending0/三个未清 native 到 Complete/processed6/pending0/last_error None，job `revoke-53d5d5a9c604a2e5b56b6c1dd9a8ff32`、source run secondary、水位 seq2/digest f2a910ba…保持。原 DB 两份 hash 独立复核原样不变。不混淆两个真实旧库，也不把默认未设置 env 的新库测试当升级证据。
历史段标题：实施证据与裁决
历史first事实：`qa/impl-ag069/final-report.md` SHA256 `836a867fd5104a3315a833a0a30ddb029f2d440a5a63c295d202e4cea6f13f22` 已完整读，所有顶层原始证据归档。storage 基线正常编译 3/10，真实旧库基线 0/1；修正新增 fixture 的水位 hash 设置后定向 27/0。三个正常编译产品变异分别 13/14、22/5、26/1，exit101，恢复 cmp0。首次 clippy 仅新测试 len_zero，等价 !is_empty 修复后全部重新验证。
历史first事实：R1 授权 cleanup_fixpoint 唯一测试末块，R2 授权 curriculum_cleanup_v42 的 assert_blocked 末块：原 Admin 再调用必须立即 Failed 的断言与完整有界重扫不相容。保留 Worker Failed、不出现 Complete、整轮终态精确 Failed/错误/身份/水位和原未知正文；未授权区块逐字不变。卡原授权 AG068 单个旧 Failed 测试改为合法恢复，其他九项与 helpers 不动。产品返修 0，旧测试范围裁决 2。
历史first事实：原 workspace **1283/7** 的两项旧语义断言与五项 HTTP EPERM 分开记录；R1/R2 定向通过后，R3 同完整命令环境复跑 **1290/0**，build/两 smoke 0。默认 CLI readiness exit1 的原日志单独不能证明 EPERM，不伪称其 stderr；环境重跑实际 0。原失败、脚本设置错误与恢复证据保留。
历史first事实：卡 SHA256 `b988de7f2245c7b495f5fa013eb930c0b48505d14de45725feb1dddf67bd5c30`；R1 `f5a48d3cb025a9bcf36eb72da368ce2f8833d4f3eb0fd3612e20d41996b2fee2`；R2 `c97c91f5bb98b7fc680f9bd0c73e82349cd960ec4a6e070dc59f365f710d4c41`；R3 环境 `32e25b40c92411cc5c059b27a27bdf6d5d97b709fe28d26ca071da6baf72ae40`。
历史段标题：结论与边界
历史first事实：本地 verified 仅 E08/E16.5，§11 L1069–1081 特别 L1077、§11.3 L1110–1112、§11.5 L1132–1134、E08 L1295/L1299，V017/V018/V038 的本地持久清理恢复子范围：在现有可信控制事实下，旧 Failed 通过显式 Admin 重扫完整 frontier，在已支持分类和受控 export 副作用验证完成后真正 Complete，分页重启可续且费用/历史/撤销不回退。
历史first事实：EXISTS 不代表常量或有界 SQL 扫描成本；文件系统与 SQLite 非原子；ensure_logical_block 的既有 tombstone 正文绑定不在本卡增强；支持分类之外仍 Failed。不声明未知 schema 自动修复、缺 anchor 恢复可信、所有 Pending 读门、完整备份链、物理擦除、第三方销毁、完整 V076 导入学习链、真实模型/收益/发行。
历史first事实：回滚参考实际父 212c784，仅撤销实现，不恢复明文或回退任何水位、账单、审计、已发生动作。无冲突，admin 未使用。AG069 从未请求外发，无本任务自动审批拒绝；遵照 AG071 公共外发拒绝后的统一暂停保留本地，待授权。若 main 前进，必须重基并全新复验再合并。
原未归属项的有限补述：AG069旧Failed续清的历史局部结论段，不扩大为完整清理范围。
原未归属项的有限补述：AG069仅在已有可信控制事实和已支持分类下，Admin重扫完整frontier后真Complete并可分页重启；费用/历史/撤销不回退。
原未归属项的有限补述：AG069不证明有界SQL成本、文件系统与SQLite原子性、未知schema修复、缺anchor恢复或物理擦除；未知支持仍Failed。
原未归属项的有限补述：AG069本地旧Failed续清结论段，和PR118后续真实merge角色分列。
原未归属项的有限补述：AG069只在受控支持分类下Admin续清到Complete并保持费用/历史/水位；不授unknown分类自动恢复。
原未归属项的有限补述：AG069文件系统/SQLite非原子、EXISTS非有界成本；不增强原tombstone绑定或声明物理擦除/全部Pending读门。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E08/E16.5；E08.ag069_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-069.md`，文件SHA256 `b988de7f2245c7b495f5fa013eb930c0b48505d14de45725feb1dddf67bd5c30`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag069-current-ddde-9edf-ROOT-direct16-run-r1-20261003/full8-workspace-r1-ROOT-closed-direct-case-review.json`，文件SHA256 `3ec8396d065404f8bfffb96b0473435511c90b926d7fdcfcab96dc241f233577`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag069-52de2d7.log.test`，文件SHA256 `70a6182d9a44503467bd51225414e6e771f58ad76f13f492a18a7a2b0bad50e2`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag069-current-ddde-9edf-ROOT-direct16-run-r1-20261003/parent-adversarial-two-r1-ROOT-closed-direct-case-review.json`，文件SHA256 `8d0f8a3899507308a6c5eba1cc0826cde149cd70de72ec2021e482a5834cda2e`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag069-current-ddde-9edf-ROOT-direct16-run-r1-20261003/head-adversarial-two-r1-ROOT-closed-direct-case-review.json`，文件SHA256 `bf4eda084bfe49d5708026a67c4125094698e22ee4b8294aced51aded232c287`。
旧v4.2阶段 B03（H `47792d943c53693cb43ecec2b9e3fbda3ec58285`；实际Parent `9b9084a328b78d7376247b78cc26e75235a09afa`；原时点，本地/失败/授权角色与本节最终头分列）：
历史阶段补述：旧R1四门inner0/1300Rust；build postguard拒绝，后四门未跑、最终验收空，失败原件保。H47792d/Parent9b的clean事实不授全八或最终PR118信用，也不把旧52de的完整1290挪到此失败阶段。后继六文件/旧DB probe清单与静态计划当时仍须Root具体调度，未授编译/SQL/runtime。
精确旧阶段原像导航：`historical-original-ledger` 的 ledger:1578；各完整原LF SHA及文字映在本地R11覆盖；本补述不授新验收。
### PR #119 — AG-073（第四轮历史）
冻结M6角色：公开head b67ca370b0dca0163465bb8037bf940c2ae38b79；本地验收head b67ca370b0dca0163465bb8037bf940c2ae38b79；实际merge 06fb166b006e5479a80e04dc14998109c30b0d42；mergedAt 2026-10-04T15:01:11Z。
合并父 5ee01d1e2ebb2401fe94530ff286801239940259, b67ca370b0dca0163465bb8037bf940c2ae38b79；tree 00b78c590d369f7649061d4b3ccbd713fc79264d。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E08/verified_subscopes/9`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E08.ag073_local_scoped_acceptance；description = Completed model cache source-revocation refusal
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = b67ca370b0dca0163465bb8037bf940c2ae38b79；pr = https://github.com/acosmi/RSIAgent/pull/119；merged_sha = 06fb166b006e5479a80e04dc14998109c30b0d42；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/broker_cache_revoke_v42.rs；command = cargo test --locked --offline -p evo-engine --test broker_cache_revoke_v42；exit_code = 0
历史原证据登记：input_digest = e5dbbaccdc2e6572c6011032d654b0e92c942031d217997599fb823911688132；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-073-b67ca37.json；log_path = out/audit-20260930/qa/takeover-20261002/ag073-fixedb67ca-ROOT-R51-local-Task-accepted-scope-corrected-r2-20261003.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E08; Completed model cache source-revocation refusal software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source b67ca370b0dca0163465bb8037bf940c2ae38b79; actual merge tree equals accepted tree 00b78c590d369f7649061d4b3ccbd713fc79264d. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #119; retain source, costs, revocation history and original QA. Accepted parent 5ee01d1e2ebb2401fe94530ff286801239940259.
**历史first阶段（Codex原记录L310–348；原时点、原head与后来的公开/验收head分列）：**
历史段标题：未合并附录：AG073 本地独立验收完成
历史段标题：CTRL-AG073-R1：Completed 模型缓存即时来源撤销读门，本地独立验收通过
历史first事实：固定 head `a9893b0bcbb28d9c50bc37d5ad851bcf840ba137`，实际父/当前 main `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `3ff2c03a14e4f26709e299d871019e851cb3a488`。唯一提交，无叠放/冲突。两个白名单文件 +1049/-3 已完整读审，最终 broker SHA256 `6328e3c49081c36e3d3ea4817686dbdab937a624b911829a4ab64c4260de1d65`、1015行新20测试 `93cfb1e386a3a5417617c29bf54ec4ba6572acaf439804b32a72f83e11d267da` 与完整预审一致。交付/探针后 clean。无 PR、未 push/ready/merge，不计已合并任务。
历史first事实：product仅 existing_response 在非redacted缓存先解析并validate_against，对 Completed 通过已绑定请求的全部 source_closure 映射 run/id，使用同一当前namespace事务内现有直接与upstream判定。门位于恢复 close_budget_call_execution 之前。撤销命中固定 `model_response_source_unavailable`；超10000沿用共享精确Conflict；数据库错误不吞。旧BudgetCallRef缺失/空/损坏不能缩减读取闭包，不额外新增来源存在性/可信性校验。
历史first事实：全新 `target-ctrl-ag073`，`ctrl-ag073-a9893b0.log*` 八门实际 exit0：fmt/clippy/workspace/build/双smoke/support checker/unittest。主控独立求和 **97结果组 = 92测试二进制 + 5doc，1293/0 = 实际父1273 + 新20**；Python48。未复用实施target，jobs2仅限制构建资源，不过滤测试。
历史段标题：两项主控独立反例
历史first事实：冻结旧API源 `probe-ag073/source-broker-212c784.rs` SHA256 `9cb8c4a2de0f858ad5dcdbde2f90f0c38695bd8dae78276cdc79fa51277fecdc`；同一 append SHA256 `79c652447b6c2c87d2120cf3e04cddf438d2fe60fb0f81001e087e040fa05d13`。
历史first事实：- P1：真实结算Completed后，先撤销尚无关的合法E16 artifact，缓存仍成功（无关水位正例）；之后才注入来源→中间节点→已撤销artifact及环的迟到typed边，重开Store、Disabled transport、新clock重读必须拒绝，不再派发、不修改已发生费用/回执/审计。故意迟到边是门禁夹具，不冒称正常生产生成链。
历史first事实：- P2：真实Completed后根预算和stopgroup均停止、旧lease过期，未撤销来源的缓存仍返回旧内容（停止/过期不自动抹去历史）；实际来源begin_revoke后不做cleanup，两次重放必须拒正文。公开verify_receipt仍验证已发生事实，原费用/dispatch/调用数1保持。
历史first事实：head `ctrl-ag073-adversarial-head-a9893b0.log` **2/0 exit0**；实际父 `ctrl-ag073-adversarial-base-212c784.log` **0/2 exit101**，双方成功编译、同一source/append，父均在预期拒读断言失败，非设置/编译错，事后clean。初版root静态复核发现普通artifact不能作合法撤销source，在任何运行之前改为精确E16 envelope；原append保存 `append-before-artifact-fixture-review.rs`，不把未运行版本算失败。
历史段标题：实施证据与裁决
历史first事实：最终完整报告 `qa/impl-ag073/implementation-report.md` SHA256 `4d579564b18c08dc60d6ad9a7fd8b4f3c59534a7f8feec1b5863a6af491d65a7` 全读；65项证据清单 `evidence-manifest.json` SHA256 `07c3a6bec86bf9ef3379599e8ba4666a560f16371a101f03691ccea7794f0fe4` 逐条核对应归档大小/hash。
历史first事实：未改产品基线20项 **9/11 exit101**，固定20/0；删除门及只查primary两个变异都正常编译 **9/11 exit101**，两次恢复cmp0。恢复后4 binaries52/0（旧broker21+新20+预算门7+late_response4）。真正settled未关闭执行由原API生成，无SQL伪造该状态；缺/坏ref、手工边和坏墓碑明确为gate fixture；拒读前后同一只读SQL快照比较全部表，已发生费用不回退。
历史first事实：原完整workspace1288/5五项均HTTP listener EPERM，R1获准相同完整命令复跑1293/0；默认CLI exit1只有readiness超时日志，不能单独证明stderr EPERM，环境重跑实际0。初本地git add index.lock沙箱拒写128，经同两白名单动作批准成功，非自动审批拒绝或外发。原日志全部保留。
历史first事实：卡 SHA256 `cbae1fd0ced666fa0e2cfef9ec154a3962fd5971416c3290e12ba7ba9dc91304`；R1环境 `38fa9f0f29ba66c11122c13f9f5a88fadbff0134df3675d24ad8890c81f2c66a`。产品返修0/环境裁决1。卡允许提前解析，故坏非redacted工件现在先校验再可能close，合法Rejected/Uncertain/redacted恢复保持旧行为；没有更改新provider-await结算路径或持久wire。
历史first事实：root首次记录脚本在提权默认python3落到旧解释器报SyntaxError，未写记录；同命令后段bash独立启动并完成本次fresh全量。已明确用out/pybin/python3补记，没有重复启动或把记录脚本错误算产品失败。
历史段标题：子范围与边界
历史first事实：本地verified仅 E08/E03 的缓存读取：§11L1077、§11.3L1110、§11.5L1132–1134、E08L1299/E03L1229，V017/V018限定**本次只读事务快照可见的来源及完整上游撤销，阻止Completed缓存正文再读；保留费用和已发生历史**。缺ref不豁免；不修改账本、不退款、不伪造NotDispatched、不重派、不启动清理；公开历史receipt验证保持。
历史first事实：快照之后提交的撤销不在本次快照保证内；清理前仍可能有持久明文。无任意返回时刻并发线性化保证、不证明物理擦除/绝对脱敏/真实模型/完整typed E16 broker引用认证/完整恢复或V076/收益/发行。回滚参考实际父212c784，不回退账单、水位或审计。
历史first事实：admin未使用，无冲突。AG073遵从统一本地暂停从未请求外发，无本任务自动审批拒绝；AG071的精确公共推送拒绝及待用户问题仍有效。若main变化，须重新叠放并全新独立验收后才可合并。
历史本地a989与后来已merge的b67不是一次执行：原本地1293/97/Python48、当前b67全量与06fb合并时间分别保；两正常编译反例head2/0、旧父0/2。20缓存守卫与原完整闭包/费用/回执界限不能扩大为整V/E。
Claude后来发现#119恢复关闭之前撤销拒绝使finalized且未closed调用保留活动状态（B-H1）；这是旧二验已核实High，不由历史缓存拒读first绿掩盖，也不在本卡修产品。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E03/E08；E08.ag073_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-073.md`，文件SHA256 `cbae1fd0ced666fa0e2cfef9ec154a3962fd5971416c3290e12ba7ba9dc91304`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag073-a9893b0.log.test`，文件SHA256 `5ba89c54ea4d5d4a05c72479f20293524a4f8a0e054e0857f34ffe9df40842e0`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag073/source-broker-212c784.rs`，文件SHA256 `9cb8c4a2de0f858ad5dcdbde2f90f0c38695bd8dae78276cdc79fa51277fecdc`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag073-adversarial-head-a9893b0.log`，文件SHA256 `eb0d5dacf1394cc208d3039f2c8f93dfd131fc1f304150017da6394fd3cdc50c`。
### PR #120 — AG-078（第四轮历史）
冻结M6角色：公开head a4b1ac77f4b56c2328cff32040370c678066fd59；本地验收head a4b1ac77f4b56c2328cff32040370c678066fd59；实际merge d56e2bef5cd552e0055d532f1e1908ed24921dcf；mergedAt 2026-10-05T06:49:23Z。
合并父 06fb166b006e5479a80e04dc14998109c30b0d42, a4b1ac77f4b56c2328cff32040370c678066fd59；tree d3e55f82c32bcb65b9b81bf8555e3e511eab3659。原双父合并角色；不是本轮网络重查或重新执行。
**历史first阶段（Codex原记录L510–542；原时点、原head与后来的公开/验收head分列）：**
历史段标题：CTRL-AG078-R1：本地独立验收，未发布/未合并
历史first事实：固定 head `4a0464aa8fa726295c9eb6527560981139cd803c`，实际父及 origin/main `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `79943e6dca3e539d9fe049259e57fc1c000cff5a`。两新脚本 +1265/-0（447行只读报告、818行测试），唯一提交，无叠放/冲突。主控完整逐行预审两文件，交付后重读报告实现并核两源SHA与提交及预审相同；全量期间未向WT放文件，结束后clean。
历史段标题：主控独立复跑
历史first事实：全新 target-ctrl-ag078；`qa/ctrl-ag078-4a0464a.log*`，session67697实际结束0，完成时间2026-10-01 16:30:55 PDT。八项实际exit0：fmt、clippy -D warnings、workspace完整1273/0、rsia build、双smoke、support-scope checker及其48项Python单测。逐原Rust日志求和96组=91测试bin+5doc、0失败/忽略/筛除，等于未修改Rust的父1273。CLI model_transport=disabled/benefit_claimed=false。额外独立运行新增Python单测46/0，日志 `qa/ctrl-ag078-budget-tests-4a0464a.log`，不把它漏出验收也不混进Rust计数。
历史段标题：两个独立反例与父事实对照
历史first事实：主控自写 `qa/probe-ag078/probe.py`，SHAab7305df90ea669cc226b5ac9ab46ba23c56fca40665d7cf3cbc2835bdf90a70。head日志 `qa/ctrl-ag078-adversarial-head-4a0464a.log` 实际2/0/exit0；父事实模式 `qa/ctrl-ag078-facts-base-212c784.log` 实际2/0。父不存在新报告接口，因此父结果诚实归为旧API真实样本及边界核验，不冒称红转绿或缺符号红灯。
历史first事实：- P1 用真正旧公共Rust授权/预留/派发/结费/对账/close生成的7个scope、13个call复制数据库，经带Unicode与#?%路径逐条CLI分页，核原根金额、每call元数据、费用NULL与0、六状态、财务与执行独立、页标记/计数、无秘密输出，以及全部业务数据前后逻辑快照不变。
历史first事实：- P2 在真实已finalized正费用7的行仅破坏实际币种：坏行之前的合法prefix仍可报告且不能称全量；包含坏行的整页固定invalid_call、退出2且stdout为空。同一数据库另scope的uncertain/已close/已知9EUR仍正确报告待对账。恢复原USD后六行/本页已记账7/原根7保持。故障只在副本，原样本SHA不变。
历史first事实：真实生成器SHA59ce03b4c5ef2254570ff9101a999e832668877140d7d7a7846d2e2c876d61a3；数据库237568字节SHAd783d371b099b9c5eaa6bdd9b78ebb4932adfbdd8cc685e11d6627cae952eafd；公共事实JSON SHAbb2ff0122aebf52b6d9040a13e7e0c58c0e6b8cb26d27696fc6838d95655cad4。生成器由实施方在产品diff0旧API正常no-run0后实际1/0生成，主控读取原代码/日志并核输入，未冒称主控重新运行生成器。原数据库关闭后empty WAL及SHM一同保留。
历史段标题：实施证据与裁决
历史first事实：卡 `cards/AG-078.md` SHAc62a12f768721a042f6d272b519af78f5547b0c15dbb05e0b29e6b320c0a370f，无R裁决。最终REPORT SHAee83d2074e0c535af1fdcfe0f35d74b8eda71ca7fbfb311608b95e9879ab6f97已全文读；55项manifest SHAb30483691dbcb491e9054c959854f62a747e9801acd7aa2d6abd002b334a1c54逐真实hash/大小核实并归档qa/impl-ag078，记录qa/ag078-reviewed-delivery.json SHA743c571aca22973e86e8606cad96037efaeb323c21a62147bc5ccb87dec9ddb0。
历史first事实：实施Python首跑45/1error系authorizer误拒PRAGMA integrity_check内部FTS读取，只对该明确阶段修测试例外，没有放宽产品业务正文读取约束，修后46/0。两个AST语义变异实际44/2失败、零执行错误，恢复逐字cmp0并46/0。自测完整1273/0及fmt/clippy/build/双smoke均0，19命令全部闭合；全部原失败留存。资源低阈值后只回收已停止91个测试exe，再续build/smoke；主控验收另用全新target。
历史段标题：有限结论与边界
历史first事实：本地verified仅E04/E16.5只读预算诊断：§3.5L392、§3.6L396、§6.6L705/709、§11L1077、§11.3L1112、§11.5L1134、E16.5L1453及V008/V096.cL1770的报告子范围。mode=ro/query_only/同一BEGIN读快照、精确迁移/依赖schema校验与integrity、单billing_scope全部namespace的BINARY keyset有界页；金额十进制字符串，NULL不填0；uncertain非空actual不升已入账，closed不升费用结清；固定列不读业务正文/lease/payer，固定错误无片段或半份JSON。
历史first事实：不声明全账或跨页同快照、未返回页的关系验证、自动对账/服务授权/阶段额度、完整优化成本/净收益、真实账单/提供商、任意并发路径替换或ACL防护、所有恶意schema对抗、全V008/V096/E16.5、完整导入学习链或发行。limit+1额外行仅判断has_more，不输出或验证其业务字段；原根金额不从本页重算，数学和可超i64且不写回。SQLite普通只读可能产生WAL/SHM协调文件，不宣称零文件副作用；只保证无业务SQL写入。
历史first事实：验收时刻 2026-10-01T23:33:44.626249+00:00。无push/PR/ready/merge/admin或merged_sha，不计已合并产品/support-scope。AG071及072具体公共push拒绝仍待人类授权；AG078自身未申请外发。若main前进须重新叠放并fresh八门/反例；回滚参考父212c，保留原账单与撤销历史。WT/源码/QA全部保留。
历史first事实：本地验收manifest SHA112ac07f43b3a8f5ca0dfccd2a4a71bec5d44f4d4f332dea209340b1dd1b0623。
高风险历史 P120_TECH_SCOPE：AG078当前a4b1/P06fb旧full8及Python46有真实来源；P2仅SQL structural、public Rust API费用fixture、paid_provider_attestation=null，不是确认实际provider账单/效果。
高风险历史 P120_SETUP_REPAIRS：AG078早期九状态同billing_scope不合法，不通过改并发setter/close费用来凑；改独立scope/DB，并以真实Host/Admin settle保留9EUR mismatch，Worker finalize直接Err。SOURCE整改非产品bug/Parent-red。
高风险历史 P120_ADJACENT_NOT_BORROWED：5617–5618 Python95/17/4/14/R3 runtime属于AG079，不能转给PR120；5585 AG078自己的PASS_LOCAL_TASK与明确复用范围保留，不拿相邻段运输0当新全量。
高风险历史 P120_AUTH_MERGE：5611人类具体批准仅AG078固定a4b1两Python+1190B正文与条件admin；此前自动审批拒绝保，AG076外发仍独立pending。5620–21实际ready/merge/two-parent exact-tree并列，空checks不称CI通过。
高风险历史 P120_PROTOCOL_DEVIATION：AG094/Claude二验已登记PR120默认merge标题无skip-ci并自动触发RustCI success；保为历史偏离，不以5611批准推导该偏离合规/新Actions授权。
当前a4b1/父06fb阶段的独立Rust1363/102、Python46与旧4a046/父212c阶段1273/96分列；新增五条R5运行四个whole各1pass/0fail，paid_provider_attestation=null、P2仅SQL structural。公共Rust费用fixture不是实际provider账单。
早期九状态复用同billing_scope属于不合法setup；改独立scope/DB，真实Host/Admin settle仍保9EUR mismatch，Worker finalize直接Err。不能改并发setter/close费用凑健康样本，也不以setup修正作Parent-red产品修复。
具体人类授权只固定a4b1两Python+1265/-0及1190B正文、符合条件admin；原自动审批拒绝保。ready/merge真实native0，空check rollup不等CI绿；双父06fb+a4b1、tree d3e55f82c32bcb65b9b81bf8555e3e511eab3659严格等验收tree。相邻AG079的Python95/17/4/14不能转记PR120。
原未归属项的有限补述：AG078只读预算报告的局部结论与报告边界，随PR120节保留。
原未归属项的有限补述：AG078以ro/query_only和同一BEGIN快照报告单billing_scope跨namespace有界页；NULL不填0，uncertain/closed不升级费用状态，固定错误无秘密或半份JSON。
原未归属项的有限补述：AG078不宣全账、跨页同快照、自动对账或业务授权；limit+1只判has_more，普通只读可能有WAL/SHM协调文件，仅保无业务SQL写入。
原未归属项的有限补述：AG078原本地H4a0464aa/P212c7848/tree79943e6d、两脚本+1265/-0（447报告+818测试）与clean保，非后来PR120验收head角色。
原未归属项的有限补述：AG078主控独立复跑段，不能借标题泛称全部历史运行已验。
原未归属项的有限补述：AG078两个真实父公共API边界控，父/头均正常通过的共同边界不作Parent红。
原未归属项的有限补述：AG078旧真实API生成7scope/13call复制库逐页查，NULL/0/六状态、财务与执行独立、全业务快照不变，无秘密输出。
原未归属项的有限补述：AG078只在副本坏币种；合法prefix不称全量，坏整页invalid_call exit2/stdout空；9EUR其它scope与恢复USD后6行、页/根7事实保。
原未归属项的有限补述：AG078生成器59ce03b4、237568B库d783d371、公共facts bb2ff012原SHA保；实施者旧API no-run0后1/0生成，Root只核原件未重运行，empty WAL/SHM保。
原未归属项的有限补述：AG078实施证据和报告合同裁决段，保首次错误及精确例外。
原未归属项的有限补述：AG078Python首45/1error只因FTS integrity_check的authorizer夹具，修后46/0；两AST变异44/2真实语义败，产品恢复cmp0，1273/0及19命令闭合。
原未归属项的有限补述：AG078有界预算诊断的有限结论段，不授真实账单。
原未归属项的有限补述：AG078单BEGIN/BINARY keyset有界页、十进制金额与NULL语义保持；uncertain/closed不等结清，固定列不读业务正文/lease/payer。
原未归属项的有限补述：AG078只无业务SQL写入，不宣零文件副作用、全账、跨页同快照、自动对账或服务授权；lookahead只has_more。
原未归属项的有限补述：AG078原本地manifest112ac07f的历史身份，不能替当前96索引或后来PR120 head输入。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E04/E16.5；未入当前98单目标索引：Python预算报告。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-078.md`，文件SHA256 `c62a12f768721a042f6d272b519af78f5547b0c15dbb05e0b29e6b320c0a370f`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/ctrl-ag078-4a0464a.log.test`，文件SHA256 `7aa7eab42aa8dce9e281b7f3b3df615b1afe10093b6334887dbf035d4271733c`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/probe-ag078/probe.py`，文件SHA256 `ab7305df90ea669cc226b5ac9ab46ba23c56fca40665d7cf3cbc2835bdf90a70`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/ctrl-ag078-adversarial-head-4a0464a.log`，文件SHA256 `97f24a281dc041f6ace0ca5411edf90cf6b4c11e050aac121c12a089aa189031`。
### PR #121 — AG-079（第四轮历史）
冻结M6角色：公开head d0958acfdd234007056aec09277cca676b87dd52；本地验收head d0958acfdd234007056aec09277cca676b87dd52；实际merge 9f0c40215a2d7a67ba66e5427cee71ca5228bde1；mergedAt 2026-10-05T14:49:57Z。
合并父 d56e2bef5cd552e0055d532f1e1908ed24921dcf；tree 7c512b5b7e318d7b4a4a4096a2ca365ddb7e0e0f。单父合并偏离另列，不补造第二父。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E03/verified_subscopes/2`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E03.ag079_local_scoped_acceptance；description = Copied diagnosis RuleHypothesis structural input refusal
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = d0958acfdd234007056aec09277cca676b87dd52；pr = https://github.com/acosmi/RSIAgent/pull/121；merged_sha = 9f0c40215a2d7a67ba66e5427cee71ca5228bde1；pr_state = merged
历史原测试登记：test_entry = crates/evo-core/tests/diagnosis_hypothesis_structure_v42.rs；command = cargo test --locked --offline -p evo-core --test diagnosis_hypothesis_structure_v42；exit_code = 0
历史原证据登记：input_digest = 3e11220b7e82ca7212f0839a1fc849fd172e0c6789b0b5e137d89b9d91cd661f；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-079-d0958ac.json；log_path = out/audit-20260930/qa/takeover-20261002/ag079-currentd56e-headd0958-ROOT-all18-necessary-SC0-finite-Task-PASS_LOCAL_TASK-and-only-epoch-release-r1-20261005.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E03; Copied diagnosis RuleHypothesis structural input refusal software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source d0958acfdd234007056aec09277cca676b87dd52; actual merge tree equals accepted tree 7c512b5b7e318d7b4a4a4096a2ca365ddb7e0e0f. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #121; retain source, costs, revocation history and original QA. Accepted parent d56e2bef5cd552e0055d532f1e1908ed24921dcf.
**历史first阶段（Codex原记录L472–509；原时点、原head与后来的公开/验收head分列）：**
历史段标题：2026-10-01T22:37:02.419242+00:00 CTRL-AG079-R1：本地独立验收，未发布/未合并
历史first事实：固定 head `5c13c35bd6ca337cde3ec70b329b1c3ae9647a4e`，实际父及 origin/main `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `748eb0128923bb1c3b37bb5b4dd1da9af1a08ecb`。唯一提交、无需叠放、无冲突。两白名单文件 +650/-0：统一 diagnosis 出口增加3行原 validate_hypothesis 调用，新测试647行。没有更改旧断言、公开类型、serde、依赖/锁、迁移或生产接线。
历史first事实：主控此前完整逐行读取全部产品和新增测试，交付后重新核产品diff和两文件hash，分别为43c1b81f213dc4161a81e7f1b767ca2245e2f243cb4731b5968021bc01da3da3、05ba94feb31b97808f6a4d27c5312022fa8cfab7dc5f030e6ce7a468e3335d07，与预审完全相同；探针前后工作树clean。
历史段标题：独立复跑
历史first事实：全新 target-ctrl-ag079，日志 `qa/ctrl-ag079-5c13c35.log*`；session6046于2026-10-01T22:34:23Z闭合。八门各exit0：fmt、clippy -D warnings、完整workspace、rsia build、双smoke、support-scope checker和Python unittest48。主控逐结果行求和1290通过/0失败/0ignored，97组=92测试binary+5doc，正好是父1273+17。CLI明确model_transport=disabled/benefit_claimed=false。原始日志均保留。
历史段标题：主控独立反例
历史first事实：冻结旧API源 `qa/probe-ag079/source-optimization-212c784.rs` SHA4e06d0cdafcfe829a89dfb2b1c48fb04f57b4ad382a262f0db71f67f02f9aef1，append SHA6c5f252370cf791fa57f58e32d8665db4184885dd7708a5af48c6ba1fedf7981。实际父 `ctrl-ag079-adversarial-base-212c784.log` 正常编译后0通过/2失败，exit101；head `ctrl-ag079-adversarial-head-5c13c35.log` 正常编译2/0exit0。两树探针后clean，无缺符号编译错误冒充红灯。
历史first事实：- P1 NotSelected分支带旧行为不读取的非法receipt，同时提供跨support/counter同ID不同有效摘要；四种hypothesis、JSON往返、三次重复均固定Conflict。合法同ID同摘要冗余原样保留，未错误收紧旧receipt优先级，输入字节不变。
历史first事实：- P2 缺receipt和合法truncated两条早返，分别带秘密非法behavior digest或反例ID；均按原Invalid固定文本拒绝，不回显秘密。None与有效假设仍保留Uncertain/ContextMismatch及相同输出字节。
历史段标题：实施证据与裁决
历史first事实：最终报告 `qa/impl-ag079/final-report.md` SHA03d3a0484a576e9cc92d202f1834f78070bc1ce658776b1e9fdca2b8c922fd98已全文读；72个交付证据逐实际hash/大小复核归档，索引 `qa/ag079-reviewed-delivery.json`，原manifest f465c5f8587aba7c67e8b5102b0799e1d27bfadc47ccd1fe0d5b62fd9856cb50。
历史first事实：实施基线no-run0后8/9exit101，九早返×四kind×24坏形状=864cells均重复真实暴露；修复新17/0。98条有效golden由真实父程序生成，共66540字节，SHAc322250c1ccf9587186791beb3c477ef0736e677df91799a8539b4dad3102801，父/head实际cmp0。两变异正常编译后分别96/96和28/96坏cells抓住，测试结果各0/1；实际结束后恢复cmp0。恢复core256/0。
历史first事实：实施原全量1285/5只因HTTP listener EPERM；R2同完整命令环境复跑1290/0，没有筛选或改源码；fmt/clippy/build/双smoke全0。原optimization结果曾口头误报18，已按日志纠正4，邻组skill_edit14，总256未变；辅助提取列号错误原诊断保留。此次主控fresh八门全部独立通过。默认local git add index.lock EPERM128后，同精确本地add/commit获工具批准，不是外发拒绝。
历史first事实：卡 ecc7fceceee4f461f259a74fadac750b523a766977b711773204106aa288064e；R1 a0276fd555e881a4f564171d0dbc31e5e24b928071ed5a4e866aa6d0c0ab1ede仅将原§6.4 L662误引更正L664；R2 3e407a4fc1a96996daae74ec45f96c49ee0c81967bbbbc88331b40031c990e68仅环境复跑。无主控产品返修、无白名单扩展。
历史段标题：有限结论与边界
历史first事实：本地verified仅E03纯诊断helper结构门全出口一致：§6.4.1L670–684、§6.4L664、§11.5L1132、E03L1225–1229、V087.a/b的输入结构/原行为子集。将要复制的假设须通过原结构校验；有效输入、None、原分类与前置错误语义保持。
历史first事实：不可声明引用存在、权限、事实真实性、可信应用与因果、生产消费者、Host任意诊断登记已修复、持久依赖/撤销全链、完整V087/E03或导入学习链、模型/统计收益和发行。真实golden只是合成helper输入，不是可信应用证据。
历史first事实：没有push、PR、ready、merge或admin；无merged_sha，不计已合并产品或support-scope记录。AG071产品/AG072台账具体公共推送各有独立审批拒绝待人类授权；AG079自身未申请外发，不冒称被拒。若main前进须重新叠放并fresh全量及反例。回滚参考父212c；保留旧证据、费用和撤销事实。全部源码/WT/branch/QA保留，只回收已停止可重建编译缓存。
历史first事实：本地验收manifest `qa/ctrl-ag079-local-manifest.json` SHAa8bc859b66d4626dac746cd6a4e508865de107d75605a35cc099ef75413377e5。
原未归属项的有限补述：AG079本地first的范围与边界段，保局部helper结论和全部不支持范围。
原未归属项的有限补述：AG079仅验证E03纯诊断helper结构门的全出口一致；有效输入、None、原分类与前置错误语义保持。
原未归属项的有限补述：AG079不证明引用存在、权限、事实真实性、可信应用因果、生产消费者或完整V087/E03；golden是合成helper输入。
原未归属项的有限补述：AG079局部helper有限结论段，不授生产应用因果。
原未归属项的有限补述：AG079全出口结构门保持有效/None与前置错误语义，仍只E03诊断helper子范围。
原未归属项的有限补述：AG079不证明事实、权限、生产消费者或完整学习链；真实golden为合成输入，非可信应用证据。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E03；E03.ag079_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-079.md`，文件SHA256 `ecc7fceceee4f461f259a74fadac750b523a766977b711773204106aa288064e`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag079-currentd56e-headd0958-parentd56e-ROOT-unfiltered141-full18-finite-runtime-r2-20261005/03-full8-workspace-local-loopback-environment-retry-r2/command/stdout.raw`，文件SHA256 `ff3df84178645a05419b3b0ee61afc807d0f8e7b7414dcefed0e7696ca1d06a9`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag079-currentd56e-headd0958-parentd56e-ROOT-unfiltered141-full18-finite-runtime-r2-20261005/03-full8-workspace-local-loopback-environment-retry-r2/command/stderr.raw`，文件SHA256 `efeebf531076c467d273deff7cd4579b92be16ee1fc70b1e709f336a31ba17e4`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag079-currentd56e-headd0958-parentd56e-ROOT-unfiltered141-full18-finite-runtime-r2-20261005/18-head-expanded-whole-B/command/stdout.raw`，文件SHA256 `cdf0065249f9c5eb7f338fcbf3bdd203c5a229139e045cf16bef8548448a3960`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag079-currentd56e-headd0958-parentd56e-ROOT-unfiltered141-full18-finite-runtime-r2-20261005/18-head-expanded-whole-B/command/stderr.raw`，文件SHA256 `a53379e52a4d78bc874f8c032b1ce6bea5e86277895473ee8ecc568dc33994d9`。
### PR #122 — AG-081（第四轮历史）
冻结M6角色：公开head 0e8ff93bfd72805e169ac99879dc0a3dd6c99031；本地验收head c2f89f42ebb06703ee70af96fd5cf7e56082c722；实际merge 90d3dcbef822b17abaffffbcc866b863336c4024；mergedAt 2026-10-05T21:11:00Z。
合并父 9f0c40215a2d7a67ba66e5427cee71ca5228bde1, 0e8ff93bfd72805e169ac99879dc0a3dd6c99031；tree a1f45196ab08bd0aaac4a401bf49d20e8e9d33c6。PR122公开/验收三方例外另列。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E13/verified_subscopes/3`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E13.ag081_local_scoped_acceptance；description = Monitoring/consolidation original report and execution input binding
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = c2f89f42ebb06703ee70af96fd5cf7e56082c722；pr = https://github.com/acosmi/RSIAgent/pull/122；merged_sha = 90d3dcbef822b17abaffffbcc866b863336c4024；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/monitoring_evidence_binding_v42.rs；command = cargo test --locked --offline -p evo-engine --test monitoring_evidence_binding_v42；exit_code = 0
历史原证据登记：input_digest = aac9c5cffd8b1427c544d8610aee080e77fec741e7105da5de6528eaa0ae40d0；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-081-c2f89f4.json；log_path = out/audit-20260930/qa/takeover-20261002/ag081-current9f0c-headc2f89f42-ROOT-all12-necessary-SC0-finite-Task-PASS_LOCAL_TASK-and-only-epoch-release-r1-20261005.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E13; Monitoring/consolidation original report and execution input binding software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source c2f89f42ebb06703ee70af96fd5cf7e56082c722; actual merge tree equals accepted tree a1f45196ab08bd0aaac4a401bf49d20e8e9d33c6. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #122; retain source, costs, revocation history and original QA. Accepted parent 9f0c40215a2d7a67ba66e5427cee71ca5228bde1.
**历史first阶段（Codex原记录L588–596；原时点、原head与后来的公开/验收head分列）：**
历史段标题：AG081 本地独立验收（未 PR／未合并） 2026-10-02T06:46:33.633678+00:00
历史first事实：- E13 / §6.7.3 L751 / §11.5 L1126–1132 / V091.c L1728 / V094 L1748，有限 original-report projection 与实际 Skill/manifest 一致性；无全E13/真实效果/发布声明。
历史first事实：- 原卡 AG-081.md SHA7807756f7495c83eca30f0f116631d2394833b1a274282245887e191e1a5c3fb；R1 SHA24a7bbf1f04fdc780ec46c9b788a2cced8951ed64dd5f90688004777e1b7dfe5；fullwindow SHA3bd4f6c6791f5ee9e48282a017c5fc605171bfafc302f078b75bcae994139024。
历史first事实：- 基线212c78488b403397eaebaa65a6d280b9c6be3899 → head620c64528f29385c6a29da73a2828a464f5d11e0/tree96a2b7cb2553775e7e58c8cdbff8884c1cfe3782；main未前进，无rebase/conflict；PR/merged_sha/mergedAt/tree合并核验均 pending，无admin。
历史first事实：- 根fresh全部8actual0，1291Rust/0，92binary5doc，48Python；两个自写反例 head2/0、actualparent0/2，都正常编译，20filtered。前后source205/204与commitblob一致/WTclean。验收详qa/ctrl-ag081-acceptance-20261002.md SHA6df2984a8372be80964bbe92109322f7c538f364580fe033fea90c46b4085b53；JSON169e6ba0；inputa22771c5。rootfresh/helper/argv/actualtoolclosed原始证据路径见该自包含包。
历史first事实：- 首head17/1只新fixtureSQL已有约束，R1修该新fixture，不改旧test；wrapper126和interruptedunknownpartial保留。两正常编译mutants真实red→精确restoregreen/3completeparentheadbytegoldens/nineold101green保留，完整owner22+35archive。root静态QA标签/helperdigest/optionalENDmarker修正原稿保留，无产品修补或假结果。
历史first事实：- 所有WT/源码/DB/QA及失败targets保留；外发原具体授权问题未解，本地验收不替代授权。回滚parent212c。持续目标仍active，新实施不收尾。
原first620c/父212c与最终本地叠放验收c2f89f42ebb06703ee70af96fd5cf7e56082c722分列；公开0e8ff93bfd72805e169ac99879dc0a3dd6c99031父d56e，不是已验head。三方merge到9f0c后90d树a1f45196…等本地c2f89的树；不能把公开head当已测，默认标题/Actions触发偏离保。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E13；E13.ag081_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG081-ROOT-current9f0c-headc2f89f42-full8-two-independent-unfiltered141-finite-SOURCE-only-R1-20261005.json`，文件SHA256 `c13554b147604390c8f841848d764cf230c47f09b48cadc9ab2aff04cdc6148b`。
- 原卡导航：`out/audit-20260930/cards/AG081-ROOT-current9f0c-headc2f89f42-full12-frozen-runtime-R2-20261005.json`，文件SHA256 `ce34ff5a2128c1cd06da55c8690c54a0ad19421762dd62999e9c2dd9de46b175`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag081-current9f0c-headc2f89f42-parent9f0c-ROOT-unfiltered141-full12-finite-runtime-r1-20261005/03-full8-workspace/command/stdout.raw`，文件SHA256 `3fe40c2a0d6e22978652d728c0969e15cfa6c6907fdab8fc8ed44cd305094b71`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag081-current9f0c-headc2f89f42-parent9f0c-ROOT-unfiltered141-full12-finite-runtime-r1-20261005/03-full8-workspace/command/stderr.raw`，文件SHA256 `761e22156fa3065c619728f8732bec5f42ef809244cae9b1cf9bf536df1e2ce1`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag081-current9f0c-headc2f89f42-parent9f0c-ROOT-unfiltered141-full12-finite-runtime-r1-20261005/10-parent-expanded-whole/command/stdout.raw`，文件SHA256 `7cc93b60247b5977758c91e2c00d8504027fdd55c204cdf7859e29f6e742fa56`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag081-current9f0c-headc2f89f42-parent9f0c-ROOT-unfiltered141-full12-finite-runtime-r1-20261005/10-parent-expanded-whole/command/stderr.raw`，文件SHA256 `3f71cbe8328af41702604b85e32457b4ed7bec07440e229740b0d2029b249d0d`。
### PR #123 — AG-080（第四轮历史）
冻结M6角色：公开head 4ed8d9d886b8e5c1f45d2477478de53fe5ee6348；本地验收head 4ed8d9d886b8e5c1f45d2477478de53fe5ee6348；实际merge 618ad834ff73e878898fb64681a4efc323b84ac0；mergedAt 2026-10-06T11:36:08Z。
合并父 90d3dcbef822b17abaffffbcc866b863336c4024；tree db94c2ae2addeb25ddeaf470454940c1a3473e5a。单父合并偏离另列，不补造第二父。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E05/verified_subscopes/3`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E05.ag080_local_scoped_acceptance；description = QueryBook and Evaluator frozen plan binding
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 4ed8d9d886b8e5c1f45d2477478de53fe5ee6348；pr = https://github.com/acosmi/RSIAgent/pull/123；merged_sha = 618ad834ff73e878898fb64681a4efc323b84ac0；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/evaluator_plan_binding_v42.rs；command = cargo test --locked --offline -p evo-engine --test evaluator_plan_binding_v42；exit_code = 0
历史原证据登记：input_digest = a47bf32311931541139dd5f031d3f59c3c3af782e43b0f051da3cf08155acef2；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-080-4ed8d9d.json；log_path = out/audit-20260930/qa/takeover-20261002/ag080-current90d3dcb-head4ed8d9d-ROOT-all12-necessary-SOURCE-RuntimeSC0-memory-only-PASS_LOCAL_TASK-and-only-epoch-release-r1-20261006.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E05; QueryBook and Evaluator frozen plan binding software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 4ed8d9d886b8e5c1f45d2477478de53fe5ee6348; actual merge tree equals accepted tree db94c2ae2addeb25ddeaf470454940c1a3473e5a. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #123; retain source, costs, revocation history and original QA. Accepted parent 90d3dcbef822b17abaffffbcc866b863336c4024.
**历史first阶段（Codex原记录L582–586；原时点、原head与后来的公开/验收head分列）：**
历史段标题：CTRL-AG080-R2 local independent acceptance 2026-10-02T04:45:45.114283+00:00
历史first事实：- Fixed1c9801e906db07252e8dc6fd7f3ac21547cff04e/parent212c/tree77afa5d04f68b92ada0db37d6bca3f170975bbab clean/4files+1389/-3, latestfetch0/mainunchanged/norebaseconflict. Rootfresh8allactual0/1306Rust0fail/93bin5doc/Python48; fourheadprobes4/0 and sameAPIactualparent0/4 afterbothnormalcompile,7filtered. Four-sourceWT/tested/blob unchanged. qa/ctrl-ag080-acceptance-20261002.md SHAfef87c275c7bf28dd6626132ab5821a780b2c07715b711c1d5daeffa75d05969; qa/ag080-root-acceptance-20261002.json SHA5714ba6656364692e6ab89cfd036933081224d08a966d2a020cf0aace5de6ea4.
历史first事实：- Verified only oldinmemoryQueryBookshape/epoch and Evaluatorfullplanbinding/E01E05/V009V010 subrange; no persistence/trueprovider/approval/fullscope/effect/release. R1/R2 privatealias-only two realClippy101 and initialabsolute-manifest1/correctedrelative0 retain truthfulargv/results. No PR/push/ready/admin/merge. Historical9+new077/080=11localacceptedproducts and072ledger, unmerged; allWT/QA kept.
历史first事实：- AG081 ownerexclusive20directedcommands authorized afterroot69661closed; full/commitseparate.082/083staticawaitshort. Goalactive/unbudgeted, no wrap-up; exactpublic071/072approvalblocks and056workflow unchanged.
**历史first阶段（Codex原记录L755–758；原时点、原head与后来的公开/验收head分列）：**
历史段标题：ROOT checkpoint R38 — 2026-10-06T11:46:46.229605+00:00 — AG080 PR123真实合并 / current main618 / 总体进度
历史first事实：- 人类当前call_1mbzyzZ9LykS6oAmuQjVflK6回复明确批准AG080固定4ed四Rust +1389/-3、3572B正文与条件admin合并。Root具体auth1671/03d9aeb4f890fb062a9e2fe1ac6bd8234cfeb9717a9813231df40530feb2988d。606c6c/session88674→03c3aa/native0真实push到codex/ag080-querybook-plan-binding-4ed8，所有6072文件全文/full14保持，只增加origin该专属跟踪ref44→45。精确3572正文创建draft PR123并attach；实际远端head4ed/base90d/4files/mergeabletrue，actoradmin；随后ready及gh --admin --squash --match-head-commit4ed --body-file原3572，fe177b/session27422→41fab3/native0。
历史first事实：- PR123 https://github.com/acosmi/RSIAgent/pull/123实际MERGED at2026-10-06T11:36:08Z，merge618ad834ff73e878898fb64681a4efc323b84ac0，parent90d，tree db94c2ae2addeb25ddeaf470454940c1a3473e5a严格等于固定验收H4ed tree；当前main/origin/main均618，Root干净。实际fetch/ff已完成，原de7829/session3041→ee6122/native1在末端ref guard过强而失败：for-each-ref解析origin/HEAD也随origin/main变化。Root不重做fetch/merge，仅c8aa45/session74028→6bcba5/native0/PID48758只读核真实symbolic target仍refs/remotes/origin/main且原文ref相同；直接两refmain/origin-main90d→618，origin/HEAD仅间接解析变化，其它42ref不变；45总数。Root4已批准Main文件逐byte等于H，其他6070文件全文/full14保真。真实postreceipt19199/09c27111c10518511eccc01cc3146619cbb6e5a2b53fb9636efbe45b76f7a836，原native1不掩盖/不虚造内部未封的stdout，不重跑产品/SourceSC。
原本地1c980/父212c与当前公开验收4ed8/父90d分列。其四旧API控双方正常compile，原本地head4/0、父0/4；新公共授权/3572B正文仅自身四Rust+1389/-3。实际squash命令与Claude读回single/rebase提交角色不混，合并tree等4ed8；ref校验origin/HEAD间接解析导致的native1原件保。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E01/E05；E05.ag080_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG080-ROOT-current90d3dcb-head4ed8d9d-full8-parent11-head11-unfiltered141-finite-SOURCE-only-R1-20261005.json`，文件SHA256 `193cd8dba4fac3a3d769f6024638699acd5a9f9f19dcbb22a626dfc4de994145`。
- 原卡导航：`out/audit-20260930/cards/AG080-ROOT-current90d3dcb-head4ed8d9d-fresh-all12-whole11-unfiltered141-explicit-control-overlay-runtime-only-R4-20261005.json`，文件SHA256 `6bf750dd7170728a70de5559b4c7be6d4c3f8382e04ede8fc03b6032ff11c74b`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag080-current90d3dcb-head4ed8d9d-parent90d3dcb-ROOT-unfiltered141-full12-whole11-finite-runtime-r4-20261005/03-full8-workspace-r4/command/stdout.raw`，文件SHA256 `ce02bd585af8eaa4deda781b319118ba351d6adbbf0ebae5731dd58f20181444`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag080-current90d3dcb-head4ed8d9d-parent90d3dcb-ROOT-unfiltered141-full12-whole11-finite-runtime-r4-20261005/03-full8-workspace-r4/command/stderr.raw`，文件SHA256 `66a6fa861b6e120508ccdd40bf109bdf9ce0dfe91c51f46add2ad3f861049052`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag080-current90d3dcb-head4ed8d9d-parent90d3dcb-ROOT-unfiltered141-full12-whole11-finite-runtime-r4-20261005/12-head-expanded-whole-r4/command/stdout.raw`，文件SHA256 `5917ae74d397658dd45208bf4cee59434cc09e992dcd63d21ecbda3a54e50646`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ag080-current90d3dcb-head4ed8d9d-parent90d3dcb-ROOT-unfiltered141-full12-whole11-finite-runtime-r4-20261005/12-head-expanded-whole-r4/command/stderr.raw`，文件SHA256 `da93ea91e38157cb1201daec93f9b69cf70069b278054c7a7fafe6566382caea`。
### PR #124 — AG-082（第四轮历史）
冻结M6角色：公开head cd19f84b85712447820b893a8c72bf7d011cd0e7；本地验收head cd19f84b85712447820b893a8c72bf7d011cd0e7；实际merge 5295769892af37988d6bc739bdfb49a834a3f7aa；mergedAt 2026-10-08T01:57:55Z。
合并父 618ad834ff73e878898fb64681a4efc323b84ac0；tree 3953fee4404251447c6bd5311adae4ed5c242ae8。单父合并偏离另列，不补造第二父。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E04/verified_subscopes/1`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E04.ag082_local_scoped_acceptance；description = Root budget authorization rollback on owned event aborts
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = cd19f84b85712447820b893a8c72bf7d011cd0e7；pr = https://github.com/acosmi/RSIAgent/pull/124；merged_sha = 5295769892af37988d6bc739bdfb49a834a3f7aa；pr_state = merged
历史原测试登记：test_entry = crates/evo-storage/tests/budget_sql_fault_rollback_v42.rs；command = cargo test --locked --offline -p evo-storage --test budget_sql_fault_rollback_v42；exit_code = 0
历史原证据登记：input_digest = 6eb66e1363dad3d81ba5a25027b5f0720071518d8af8ff36101163095c41fdfe；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-082-cd19f84.json；log_path = out/audit-20260930/qa/takeover-20261002/AG082-current618ad834-headcd19f84b-Source2-ROOT-all12-necessary-SourceSC-RuntimeSC0-PASS_LOCAL_TASK-and-only-epoch-release-r1-20261006.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E04; Root budget authorization rollback on owned event aborts software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source cd19f84b85712447820b893a8c72bf7d011cd0e7; actual merge tree equals accepted tree 3953fee4404251447c6bd5311adae4ed5c242ae8. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #124; retain source, costs, revocation history and original QA. Accepted parent 618ad834ff73e878898fb64681a4efc323b84ac0.
**历史first阶段（Codex原记录L630–636；原时点、原head与后来的公开/验收head分列）：**
历史段标题：CTRL-AG082 当前本地独立验收／latest-depth继续 2026-10-02T15:50:05.697177+00:00
历史first事实：- 固定headf2a8e93ac3010b5c0ddb2a861665c919f3b2a56e/parent9b9084a328b78d7376247b78cc26e75235a09afa/tree5eaf4feafbb20d28493e28c939bb5bcd8e1d4959 clean，唯一新测试592/0 SHA bcee58cc；owner及rootfresh8全部实际0/1285-0/93binary+5doc/Python48。root真实native85566→dbb0bf0/no-session；两探针head2/0、actualparent9b2/0（13filtered）属边界，native97749→b6d187及7965→bc450d真实闭合。源码206/205均恢复clean，独立SC无有限范围阻断。
历史first事实：- acceptance qa/takeover-20261002/ctrl-ag082-current-acceptance-20261002.md SHA07738133dae4c6abee16641b8390581f7c36aa4008ebdd959b07b628b1735451；JSON SHA670f26f9265c0376d81e9e1c0c125b13e3448815afceb1f5ea0af4ee1db14558；parent verification b11060c4。有限E04/E16.5/V008/V038 statement ABORT/五表逻辑rollback/reopen/原请求恢复幂等；无产品修复、完整E/V/磁盘满commit崩溃Linux/收益发行声明，checker input_binding_available=false保持。历史owner/root错误原样保留，parent verifier444b33 schema首错用新R1/c4b922复核，不改原件/源码。PR/merged_sha均null，未外发。
历史first事实：- compaction startup第一真源/next/start/handoff/common哈希相符，工具gh8112f0/fetch2a07d6/cargom1.98.1/Python3.12.14正常/mainorigin9b，df57249c实际42400000KiB=40.43579GiB>20。持续目标实际active。新SC gpt-6.1-sol/forknone/继承最新用户深度，无effort覆盖/实际enum未知；旧max历史不改。
历史first事实：- 下一步root保护叠放083并派R4静态、085历史fresh三命令窗口；不并行项目Cargo。AG072远端分支删除自动审批因缺这个具体branch-delete人类授权而拒绝，原待答保留，不重试；不阻挡本地工作。
AG082旧a8阶段与公开验收cd19阶段分列；宿主真实provider/ordinary P承诺等仍外部阻塞。J42关联更正与J47 EOF重试保持独立历史，不将EOF写成自动审批拒绝；实际单父合并偏离与tree验收一致分别陈述。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 当前cd19的原独立验收另列：fresh12/all24 wrappers真实0；workspace107组/1433过、Python141含原裸47且无skip/error/fail；Parent whole2兼容与Head whole2边界均0，非Parent-red修复。旧f2本地1285/98组只属此前时点，不能替代cd19。
- 当前cd19必要RuntimeSC初r1 native1/findings1仅class11缺三既有SourceSC失败原件；有限读集修正、54函数全文/AST不变，r2 native0/findings int0/12类PASS，2822文件/470目录与Root双阶段受理保。原失败未改0。
- 历史E归属/记录身份：E04/E16.5；E04.ag082_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG082-ROOT-current618ad834-headcd19f84b-Source2-after-actual-physical0-only-local-Task-and-only082-epoch-release-r1-20261006.json`，文件SHA256 `283aaec7b52395ab34f6746fd5c537435f307da32ba3a057db9ed47a2f60d198`。
- 原卡导航：`out/audit-20260930/cards/AG082-ROOT-current618ad834-headcd19f84b-full8-parent2-head2-unfiltered141-finite-SOURCE-only-r2-20261006.json`，文件SHA256 `44a54a289f9abd41296bd5dc042b7dae18e15d7dfcdeaf5299c6b2022634fcaf`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG082-current618ad834-headcd19f84b-full12-native-runtime-r1-20261006/03-full8-workspace/command/stdout.raw`，文件SHA256 `2bb1aa668185e00a08c50bc6a30b3ff84d6a2cdfc6efa8fd021ae12e948fb07b`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG082-current618ad834-headcd19f84b-full12-native-runtime-r1-20261006/03-full8-workspace/command/stderr.raw`，文件SHA256 `b5b7d901f9741bb677e8e5592c4daf0a4d0f75321d2dc6bd7a5beb4a4be0fe72`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG082-current618ad834-headcd19f84b-full12-native-runtime-r1-20261006/10-parent-expanded-whole/command/stdout.raw`，文件SHA256 `fc28bd854ffdbfa5c695e6ac094a3f527954a468d3c7d37da7de88d59e56722f`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG082-current618ad834-headcd19f84b-full12-native-runtime-r1-20261006/10-parent-expanded-whole/command/stderr.raw`，文件SHA256 `f47c34b0bf76a8f8a1ca673bfeeb5c13a0520bac49004fa4a3e3b130b31e9186`。
原阶段补述（ledger:6406，原时点；区别于最终cd19）：当前原始项目操作严格 1+9+2=12，全12 native0，各运行一次；真实 Root R3 MAIN 12f902/session31330→6c4a1b/native0、三个控制器生命周期1/1/0，前两次 Root schema/count 控制失败原件保留。完整未过滤 Rust 105组/1400通过，Python 141/same bare MCP47/全不通过项0；Parent/Head ordinary no-run0/countnull 后各 whole2绿色，Parent兼容API范围不得标作语义red。host-only closure 2fdee1/session7549→972120/native0 已真封存 index/map，Source/plan/card/epoch没有更改。
原阶段补述（ledger:6413，原时点；区别于最终cd19）：##### ROOT 2026-10-06T01:13:15.038329+00:00 — AG082 necessary RuntimeSC R2 actual FAIL3; async declaration control-only repair adjudicated
原阶段补述（ledger:6415，原时点；区别于最终cd19）：ROOT 041068/native0与ba8faa/native0完整双读实际三个Rust Source及FAIL三件：新测试584/589、P/H358/396均#[tokio::test] async fn；旧声明regex仅#[test]同步fn，提取空集。实际workspace ordinal89当前测试二原名及Parent/Head whole二原名全部完全匹配Source，二pass/所有nonpass0；其余所有原criterion均true。裁决为审查声明识别控制错误，原始names/fixture/assertions/data/API/case count/Source不修改，Parent绿不得伪造red。ROOT直接rich pin/sole wrapper和triad相等比较的只读预flight cf0b28/native1原件保留，后续按实际shape核验，不把控制错误写成产品失败。
### PR #125 — AG-077（第四轮历史）
冻结M6角色：公开head 26d3fe400ee1aad36082081eda6c60f3ee1a17f7；本地验收head 26d3fe400ee1aad36082081eda6c60f3ee1a17f7；实际merge 349879148c1f31fd1ebbee639aeff334a3762855；mergedAt 2026-10-11T01:11:17Z。
合并父 5295769892af37988d6bc739bdfb49a834a3f7aa, 26d3fe400ee1aad36082081eda6c60f3ee1a17f7；tree cb47a8c5f22bbfea3a21e63ea6830478603a3694。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E04/verified_subscopes/2`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E04.ag077_local_scoped_acceptance；description = Typed E16 budget source snapshots, cleanup and recovery
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 26d3fe400ee1aad36082081eda6c60f3ee1a17f7；pr = https://github.com/acosmi/RSIAgent/pull/125；merged_sha = 349879148c1f31fd1ebbee639aeff334a3762855；pr_state = merged
历史原测试登记：test_entry = crates/evo-storage/tests/e16_budget_binding_v42.rs；command = cargo test --locked --offline -p evo-storage --test e16_budget_binding_v42；exit_code = 0；production_consumer = none_library_only
历史原证据登记：input_digest = 4baadeaf4705ceb06cfbbe70968543f8daa553cfd5148a74dc2fb63cc2eafaad；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-077-26d3fe4.json；log_path = out/audit-20260930/qa/takeover-20261002/AG077-existing-PR125-current529-fixed26d3-ROOT-fresh12-actual-current-main-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E04; Typed E16 budget source snapshots, cleanup and recovery software/component boundary only. Library-only as of e727d1c: no production caller in engine/http/mcp/apps.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 26d3fe400ee1aad36082081eda6c60f3ee1a17f7; actual merge tree equals accepted tree cb47a8c5f22bbfea3a21e63ea6830478603a3694. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #125; retain source, costs, revocation history and original QA. Accepted parent 5295769892af37988d6bc739bdfb49a834a3f7aa.
**历史first阶段（Codex原记录L577–581；原时点、原head与后来的公开/验收head分列）：**
历史段标题：CTRL-AG077-R5 local independent acceptance 2026-10-02T04:28:59.866491+00:00
历史first事实：- Fixed1d0182a6d5300994af7167ceeed07a9d7c597ac9/parent212c/tree3945ccbc5cf0ed4c439951fe9241515ebc6eecd1 clean/unique10files+5179/-229. Independent fresh retry all8actual0/1322Rust0fail/95bin5doc/Python48; head4probes0fail/13filtered, source10unchanged. ParentoldAPI2/0boundary, newAPIq3/q4no-parentrun honestboundary. reportqa/ctrl-ag077-r5-acceptance-20261002.md SHA1df2b9911cb56d5edf5092821fc7f449defd4f1e06e037c62d96dc3313a37ad7; acceptanceJSONqa/ag077-root-acceptance-20261002.json SHA297242d1d8fa882a11a1e23b326d21b8d03aec03361e307d072f5286c6765ada.
历史first事实：- Verified only cardE04/E08/E16 localtypedbudget snapshot/live/content gate/realfees/cleanup protectedrestore/V008V017V018V038V094V096 subrange. FullE16/V076 learningchain/physicalerase/rawDBantitamper/businessauth/effect/release not proven. R1-R5/old1316-6/cargo deviation/correctcargom oldfixture failures/newheados35zero-group fresh failure retained. No rebase/main advance/PR/admin/push/merge; rollbackparent212c preservefees/ref/watermark. AllWT/QA retained. RootCargo session51273closed, nextexclusiveAG080fresh.
**历史first阶段（Codex原记录L763–772；原时点、原head与后来的公开/验收head分列）：**
历史段标题：2026-10-11T01:14:38.077627+00:00 ROOT 既有收尾 AG077 PR125 实际固定提交合并与树核对 / postpaid SourceSC 接收 / J44
历史first事实：- 继续 WRAP_UP_EXISTING_ONLY；不新增产品任务、编号、代理或派发。rsiagent-30 已删除，不重建。用户交付通过 admin 合并的会话授权继续有效，无重复公共授权问题。AG084 外部工具等待已 J42 撤销。
历史first事实：- AG077 固定26d3fe400ee1aad36082081eda6c60f3ee1a17f7 / parent5295769892af37988d6bc739bdfb49a834a3f7aa / treecb47a8c5f22bbfea3a21e63ea6830478603a3694的当前主线 fresh8及原有反例实际全部通过。Rust111组/1482通过，Python141全部发现执行通过、原bare MCP47完整；失败/跳过/过滤均0。原头whole4与主线whole2兼容性对照通过，两个normal API no-run实际0，兼容性成功不作为语义负例。
历史first事实：- 原沙箱工作区native101及1477/5结果保留；5失败均在既有本地HTTP listener 127.0.0.1:0被OS PermissionDenied。唯一原样全量沙箱外复验实际0，不改源码/断言/测试范围。原stderr只有已观察blocks392→280，逻辑139279B/73ba9d9e及其余13metadata完全相同；有限pair与先前启动前失败保留，不作未来metadata豁免。
历史first事实：- ROOT实际19d5aa/native0接受当前完整12项，Task14342721/fda28ff3；旧独立静态审查仅以十文件字节完全相同明确保留，不将旧运行结果重标为新。提交相较旧945唯一基线变化为已合并SQL rollback测试，当前223REG/44DIR与私有反例443REG/88DIR前后保护通过，ROOT未手工编辑产品。
历史first事实：- 专属已公开分支精确lease从945更新26：295a34/native0。冻结2602B/9ceb9642 PR正文通过GitHub连接器实际更新，ready实际成功；17683d/session53499→6a4b7f/native0按admin固定26合并，标题含skip ci。真实GitHub当前main349879148c1f31fd1ebbee639aeff334a3762855，父提交恰529/26，合并tree与验收tree严格相同；PR125实际MERGED/mergedAt2026-10-11T01:11:17Z。当前聊天PR附件再次确认。第四轮已合并#98–125共28项，不能把支持索引219 needs_verification或原未实现v4.2能力冒记验收完成。
历史first事实：- AG093 postpaid两处已封SOURCE的原独立SourceSC实际54d1b7/native0，ROOT cb180e/native0物理受理2915855/bfd7779e，SourceSC=Accept；源与证据lease关闭，原15测试/145断言与global env保持。RuntimeSC/Task仍未通过，新r14未启动。现需按实际新主线及既有前置增量顺序处理，不能使用旧529的整合假设。
历史first事实：- 当前没有旧r201实施角色活跃；只由ROOT推进既有收尾验收。没有新增角色或误报后台持续运行。剩余已有增量继续核实当前主线整合、验收和条件合并；AG092等实际公共拒绝原件保留，不藏入另一PR、不换通道绕过。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E04/E08/E16.1/E16.5；E04.ag077_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-077.md`，文件SHA256 `fed505d275af29989747e1d9584392ab52acae5b5941b85414b1ecd8978c6de5`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG077-existing-PR125-current529-fixed26d3-ROOT-fresh12-actual-current-main-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/all12_complete_actual_records/2/current_complete_raw/stderr_full14/original_UTF8`，原载荷SHA256 `c8e7198cdb245d3c1d77c114bd47174fa0e4019599a680099cee7db16167e11c`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `fda28ff3071a8ac3064752700831a86b9b43302ba442de325b9d42ab2a772ecd`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG077-existing-PR125-current529-fixed26d3-ROOT-fresh12-actual-current-main-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/all12_complete_actual_records/2/current_complete_raw/stdout_full14/original_UTF8`，原载荷SHA256 `8cf619a668ef5306ea1a41f2991febce26bf287e2edc228af6bd2c5b7b698477`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `fda28ff3071a8ac3064752700831a86b9b43302ba442de325b9d42ab2a772ecd`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG077-existing-PR125-current529-fixed26d3-ROOT-fresh12-actual-current-main-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/all12_complete_actual_records/11/current_complete_raw/stderr_full14/original_UTF8`，原载荷SHA256 `6d658226a3c740e38682f4a318b5366fe1c4ec2cb76dcedfe26aa4b39500b5d4`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `fda28ff3071a8ac3064752700831a86b9b43302ba442de325b9d42ab2a772ecd`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG077-existing-PR125-current529-fixed26d3-ROOT-fresh12-actual-current-main-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/all12_complete_actual_records/11/current_complete_raw/stdout_full14/original_UTF8`，原载荷SHA256 `b8b9196cdcf145be8d80972d5710b012fafcaf54943f389a7a0d6560bc5335a6`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `fda28ff3071a8ac3064752700831a86b9b43302ba442de325b9d42ab2a772ecd`。
旧v4.2阶段 B04（H `94567b5f4ed74a4d2701d491a6de1d08cf2fa6f0`；实际Parent `618ad834ff73e878898fb64681a4efc323b84ac0`；原时点，本地/失败/授权角色与本节最终头分列）：
历史阶段补述：旧945/Parent618唯一fresh12、24真实recorder/receiver均0；Rust110groups/1480passed、Python141含bare47及全部nonpass0。Parent ordinary0+whole2和Head ordinary0+whole4均绿，只是兼容/具名QA边界，不能升作Parent语义红，也不能拿后来26阶段1482替原12阶段1480。smoke/support counts仍null、input_binding=false/needs219保；只旧本地固定头信用。
精确旧阶段原像导航：`historical-original-ledger` 的 ledger:6687；各完整原LF SHA及文字映在本地R11覆盖；本补述不授新验收。
### PR #126 — AG-084（第四轮历史）
冻结M6角色：公开head 3c5ae3d6dacf74b7b91a7bf5527226c561ca688c；本地验收head 3c5ae3d6dacf74b7b91a7bf5527226c561ca688c；实际merge ea02f116fe2172dcba621a60bf9b141237e1ba45；mergedAt 2026-10-11T01:36:00Z。
合并父 349879148c1f31fd1ebbee639aeff334a3762855, 3c5ae3d6dacf74b7b91a7bf5527226c561ca688c；tree 331ac2b5c166adc167e83eb0aa9ffccc8a7d24eb。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E06/verified_subscopes/0`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E06.ag084_local_scoped_acceptance；description = Stored candidate and stage-fact review without granting approval
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 3c5ae3d6dacf74b7b91a7bf5527226c561ca688c；pr = https://github.com/acosmi/RSIAgent/pull/126；merged_sha = ea02f116fe2172dcba621a60bf9b141237e1ba45；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/candidate_review.rs；command = cargo test --locked --offline -p evo-engine --test candidate_review；exit_code = 0
历史原证据登记：input_digest = 11fdf14efc51e39817a21ca5b34bcceb25ec82f8b3d738f94082f000285fff0f；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-084-3c5ae3d.json；log_path = out/audit-20260930/qa/takeover-20261002/AG084-existing-current349-fixed3c5-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E06; Stored candidate and stage-fact review without granting approval software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 3c5ae3d6dacf74b7b91a7bf5527226c561ca688c; actual merge tree equals accepted tree 331ac2b5c166adc167e83eb0aa9ffccc8a7d24eb. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #126; retain source, costs, revocation history and original QA. Accepted parent 349879148c1f31fd1ebbee639aeff334a3762855.
**历史first阶段（Codex原记录L773–783；原时点、原head与后来的公开/验收head分列）：**
历史段标题：2026-10-11 ROOT 既有收尾 AG084 PR126 实际 admin 合并 / AG077 已闭缓存清理 / AG056 scope 恢复 / J45
历史first事实：- 继续 WRAP_UP_EXISTING_ONLY，不新增产品任务、编号、代理或派发；定时 rsiagent-30 已删除。用户既有“验收通过 admin 合并，不再问授权”继续有效；不冒记全部 v4.2 能力完成。
历史first事实：- AG084 原九文件字节完全相同的既有交付由 ROOT 叠放 actual main349，固定3c5ae3d6dacf74b7b91a7bf5527226c561ca688c、parent349879148c1f31fd1ebbee639aeff334a3762855、tree331ac2b5c166adc167e83eb0aa9ffccc8a7d24eb。唯一 ROOT fresh10 已实际全0：完整八门、Rust114组/1503通过、Python141含原47bare MCP全部发现执行、完整原两反例正常API编译与whole2均通过，过滤/失败/跳过均0。原 SOURCE 静态独立审查仅对字节不变载荷明确保留，旧运行结果未重标。
历史first事实：- ROOT ec4a06/native0完整接受当前Task，966024/4d88e92f；物理复核十项原stdout/stderr/invocation、109实际Running程序及全部SOURCE/index guards。两反例各四份完整48项SQLite逻辑catalog受理：正常review前后与拒绝/stable review前后均完全相同，唯一fixture中途变化均限objects，跨profile替换和单Store FIFO第二次读取新S2墓碑得到准确拒绝。数据库文件只核metadata，未读取/哈希DB或WAL正文。初次 ROOT reader 423d56/native1系误预期FIFO变化涉及audit/sqlite_sequence；实际只objects变化，经d73598真实诊断纠正纯receiver比较，失败原件保留，未改产品/测试/门禁。
历史first事实：- 九文件+5652/-4固定3c5由e56c65/native0实际推送专属codex/ag084-wrapup-main349-3c5ae3；冻结1971B/51169fa5 PR正文，GitHub连接器实际创建#126并ready，已附当前聊天。56533c/session15681→129e6e/native0固定head admin合并；真实PR126 MERGED、mergedAt2026-10-11T01:36:00Z，actual main ea02f116fe2172dcba621a60bf9b141237e1ba45，parents恰349/3c5，merge tree严格等于验收tree。第四轮#98–126现29项实际合并；没有私有真源/out/QA或反例fixture外发。
历史first事实：- 用户授权的可再生缓存清理实际30e931/native0：仅PR125新闭三target，逐文件metadata核对后unlink6554REG、移1170空DIR，allocated估计1452584960B。285编译可执行文件及全部验收绑定文件字节/full14保持，ROOT未提交状态不变；AG084在途target明确排除。实际清后7659520000B，非系统永久空闲承诺，低于10GB不全局停止。原source/RAW/QA/DB/env及Git均未清理。
历史first事实：- AG056真实gh auth4fa6cf/native0现token scopes包括workflow，旧scope缺失阻塞已解除，仅表示可以继续该既有交付的整合/验收，不当作当前Task或已合并。AG070/075/086/092等历史具体公共拒绝原件仍保留，不换通道规避或藏入另一PR。
历史first事实：- 当前AG084 ROOT master已实际终态，不存在后台重复验证。AG093 postpaid SourceSC已Accept但RuntimeSC/Task未通过、r14未启动；其前置现有增量和其它既有未合并任务继续按实际主线处理。ROOT主checkout618及各旧工作树保护，不reset/clean；需要本次干净收尾的已完成、真实阻塞与仍待项逐一记录。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E06；E06.ag084_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG084-existing-current-main349-fixed3c5-ROOT-fresh10-runtime-r2-20261011.json`，文件SHA256 `e7a9ee7fcba5241da6423c222072a16d9cf2054d5fc6282096151e4c0cb80ef8`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG084-existing-current349-fixed3c5-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `43cf12dffbd37eb37cde43d50ce13517a23cafa563051afcbbc556f30fba8855`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `4d88e92fa79f672423c856785ceb10032039e726a773df5a06033cbdc21716b8`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG084-existing-current349-fixed3c5-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `1c2b84580b31c81a5774846f9af0571deaf628cf46b4ca4cfcd8a72ee70bc542`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `4d88e92fa79f672423c856785ceb10032039e726a773df5a06033cbdc21716b8`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG084-existing-current349-fixed3c5-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stderr_UTF8`，原载荷SHA256 `7c163480c6b3d5898cd3e9b4fe7b75545446db0ed725106e40265fe5462ca2b8`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `4d88e92fa79f672423c856785ceb10032039e726a773df5a06033cbdc21716b8`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG084-existing-current349-fixed3c5-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stdout_UTF8`，原载荷SHA256 `7f18aeda6eb34a6b5fb4f7f29e17db1fdf8f204e44dc8a6ace75ba78e6cf21f4`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `4d88e92fa79f672423c856785ceb10032039e726a773df5a06033cbdc21716b8`。
旧v4.2阶段 B05（H `9816edc16dafa0a8d5b1c185b2284b00f7f310cf`；实际Parent `618ad834ff73e878898fb64681a4efc323b84ac0`；原时点，本地/失败/授权角色与本节最终头分列）：
历史阶段补述：旧Parent618普通no-run0；最初默认listener native101保且无语义红信用。经授权的whole2实际1pass/1fail：完整JSON positive通过，review L202 actual404/expected200（HTTP404!=200），0ignored/filtered；不是编译失败冒语义红。
历史阶段补述：旧9816/Parent与main/origin618本地固定头：独立八门+Head whole2、Rust1452/109groups、Python141含bare47及Head2全nonpass0，no-run countnull；仅该epoch释放、support219保，公共授权/push/PR/merge当时仍null，不计当前发行。PR125缓存1452584960B不是此PR的1452Rust证据。
精确旧阶段原像导航：`historical-original-ledger` 的 ledger:6579；ledger:6857；各完整原LF SHA及文字映在本地R11覆盖；本补述不授新验收。
### PR #127 — AG-074（第四轮历史）
冻结M6角色：公开head 313598036461304fe1b081e91115bdbd2c779752；本地验收head 313598036461304fe1b081e91115bdbd2c779752；实际merge aa51b1102e1886cbaf7b025d37b98ca30b6e2d3d；mergedAt 2026-10-11T01:51:47Z。
合并父 ea02f116fe2172dcba621a60bf9b141237e1ba45, 313598036461304fe1b081e91115bdbd2c779752；tree 16de199ce2d66f4909b094603fd8def8207abb71。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（历史已撤出对象（不计当前98；原错误/旧复核标签仍保留））：`out/audit-20260930/qa/codex-r7-ag124/withdrawn-records.json`，JSON pointer `/records/4/record`，文件SHA256 `d71db404637255b0600c8dcdeae8ac3725cad9360e3cb65e51f558da368c19fd`。
历史原登记身份：id = E09.ag074_local_scoped_acceptance；description = Replay deepen eligibility and waits from the observed legal prefix
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = not_affected
历史原提交角色：source_sha = 313598036461304fe1b081e91115bdbd2c779752；pr = https://github.com/acosmi/RSIAgent/pull/127；merged_sha = aa51b1102e1886cbaf7b025d37b98ca30b6e2d3d；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/replay_legal_prefix_v42.rs；command = cargo test --locked --offline -p evo-engine --test replay_legal_prefix_v42；exit_code = 0
历史原证据登记：input_digest = e4686ced10ab554f49b580631321c073d2c5ea892129586a297b8ae4df3ee5a1；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-074-3135980.json；log_path = out/audit-20260930/qa/takeover-20261002/AG074-existing-currentEA02-fixed3135980-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E09; Replay deepen eligibility and waits from the observed legal prefix software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 313598036461304fe1b081e91115bdbd2c779752; actual merge tree equals accepted tree 16de199ce2d66f4909b094603fd8def8207abb71. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #127; retain source, costs, revocation history and original QA. Accepted parent ea02f116fe2172dcba621a60bf9b141237e1ba45.
**历史first阶段（Codex原记录L349–394；原时点、原head与后来的公开/验收head分列）：**
历史段标题：未合并附录：AG074 本地独立验收完成
历史段标题：CTRL-AG074-R1：正常 Deepen 共享已观察父展开规则，本地独立验收通过
历史first事实：固定head `700308e4e8a546d21f1332c48ad28666e44c6d1c`，父/当前main `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `b2b00d82d9eadb77d9a6edbe62b3f45c05c2ef8f`。唯一提交，无叠放/冲突，交付及所有root探针后clean。四文件+969/-0：core strategy+6、engine replay+7、新core测试255、新engine测试701；全部内容此前完整预审，最终四SHA逐一匹配。无PR、未push/merge，不计已合并产品数。
历史first事实：两个产品门从已观察prefix构造父集合，仅过滤已有任意子节点的正常Deepen，回放available与core决定同步，不窥未来结果、不改Recover。按既有在线exploration规则和真源共享合法性目的，把同观察前缀每正常父只消费一次提升到core/replay；真源没有逐字规定跨批每父一次，明确属主控工程推导。合法Valid子可继续加深；OOS/Censored未形成子节点不预先夺走机会。
历史first事实：最终产品hash：core `0d257897604e279b69c13ffe76a6fa4bc89da3574726bf125cdfd688bd65542b`；engine `159a5798c45e5cdca10973607aa28bfef2327c6a5af8614be78a6cb8130769c9`。测试hash：core `7d36b078e7701aa53d50a87456007b3589c98f31e5c1b8ead2c4e0a561335a6d`；engine `2a08a243364574490fb1dc2801c53a2b87a5b5d7d037e03c1e58adc569be2f7e`。
历史段标题：独立完整复跑
历史first事实：全新target-ctrl-ag074，`ctrl-ag074-700308e.log*`八门实际exit0：fmt/clippy/workspace/build rsia/两smoke/support checker/Python unittest。root逐行独立求和98结果组=93测试二进制+5doc，**1291 passed / 0 failed / 0 ignored**，与父1273+新增18相符；Python48。jobs2只控制资源。新增18内有可选旧报告生成入口默认早返，不能把默认全量这一项当真实旧程序产物证据；独立生成见下。
历史段标题：主控反例与实际父对照
历史first事实：冻结旧API源 `probe-ag074/source-replay-v41-212c784.rs` SHA `ac801442ed248371a5817f61a3c3a10d1473bc0b7433b354e0cedd1e3fabf352`，append SHA `ac618b70409e1c2d755cf26b7569b494b43145aaa574c0e9e6dbaa0bb7de3062`。同一代码两树正常编译：head `ctrl-ag074-adversarial-head-700308e.log` **2/0 exit0**；实际父 `ctrl-ag074-adversarial-base-212c784.log` **0/2 exit101**，均事后clean。
历史first事实：- P1真实sealed world有root900k→child200k；旧父另seq3故意没有transition，合法孙seq4有950k。W=1/2/4必须选[1,2,4]、cost3、无OOS、无非法父等待。实际父选[1,2,3]导致OOS，head全部正向通过；不是通过窥未来剔除缺transition。
历史first事实：- P2由真实两节点回放构成合法JSON往返前缀，再加入明确人工历史HardFailure兄弟，旧父45等待u32MAX。不同W及action顺序仍只能选合法子50/其他根60，费用/槽位正确、原prefix逐字不变。实际父选择45。人工兄弟是历史形状反例，不冒称真实新执行。
历史first事实：初版P2错误复制Valid子best/gains到HardFailure兄弟，prefix.validate失败是root夹具错；原append与完整日志保留invalid-ancestor-fixture，不算产品红灯。修正继承父链后上述最终父0/2为真正选择断言失败。
历史段标题：真实旧报告兼容补充
历史first事实：root在实际父212c运行冻结旧API generator SHA `251e596dbb1a386c753adde7805fea25682273407c4feff09c77e6f6657da0c6`，`ctrl-ag074-old-generator-212c784.log` **1/0 exit0**，create_new生成三world/三report。原件在 `old-reports-ag074/`，provenance列六hash；本次全部再次核原样。三world与实施方捕获逐字一致；report仅CPU与对应body_digest不同，其他字段/semantic_digest一致。
历史first事实：root真实旧report hash分别：affected `e566ece73a28d32f40b8c3d87b89ac40861e12ccdad7d1bfdffaec639e62412c`，普通chain `68cba2c5a730c9c7f338f4ce721ebd763f3af7bc2f5cc68333482d224d6966fe`，Recover `ff6510f0c222358bf755b6b5a14a539d1f7189da914b40ee4680637648611838`。
历史first事实：补充冻结 `probe-ag074/source-root-old-reports.rs` SHA `917a11fd0741fdf5f6753c273a136672d5f5578ac2a3ccfe77087189f7cba0f7`，仅把三个内嵌report字节/hash换成root实际旧产物，world不变。同一源码实际父 `ctrl-ag074-root-old-reports-parent-212c784.log` **1/1 exit101**（受影响旧report旧验证Ok不满足新拒绝断言，两个不受影响控制通过）；head `ctrl-ag074-root-old-reports-head-700308e.log` **2/0 exit0**。static load仍读原完整artifact，受影响verified/重复同ID重算拒绝并保持原payload，普通链/Recover保持verified/idempotent及旧CPU。原程序产物来自synthetic worlds，不是模型效果或实际修复证据。
历史first事实：具体轨迹自然变化：受影响旧[1,2,3]/BudgetExhausted/complete/950000/有score，变成[1,2]/WorldExhausted/incomplete/900000/None。未改终态/评分公式、wire/digest/engine版本；不兼容旧report同ID重算失败关闭，是明确行为兼容边界，不能宣称自动迁移。
历史段标题：实施证据及结论
历史first事实：`impl-ag074/REPORT.md` SHA `0d7e5be11ececbc9dffe00a5cd328837fba0ff0d596da02d0db2e699908b9b44` 和20项 `command-results.json` 全部已读；41项manifest SHA `5f484241862c0c1829c46445c804a5fc60cf304a19ac3dda05bb9db07348cf56` 逐条实际大小/hash核对归档，包括六旧JSON。未过滤原失败。
历史first事实：自测最终core基线0/2、engine1/3均正常编译；新core7/0、engine11/0。删core门变异3/4、只删replay available门变异0/1抓公开waits泄漏，两次恢复cmp0，旧回归core29/0、engine79/0。原全量1286/5仅五HTTP bind EPERM；R1同完整workspace1291/0。原CLI仅readiness超时，不能凭其日志称stderr EPERM；相同环境复跑双smoke0。没有外发自动审批请求或本任务工具拒绝。
历史first事实：卡 `ee6487be89271262896ef9f63869c23a25b46d86512260462a47838cad72a6f6`；R1环境 `55dcc35a9352a8ad21dcb41f983063fe69a6af1081578d48a760a869c446a2cc`。产品返修0、环境裁决1，旧测试零改动。
历史first事实：本地verified仅E09/E10正常Deepen已观察前缀合法性：§7.1L785、§7.1.1L791–793/797/805/807、E10L1321–1327；V020/V022/V023/V025/V081/V086的有限本地子范围。不声明完整available_actions、一般PolicyStop/WorldExhausted/complete_support合同、生产Recover来源/修复能力、旧报告迁移覆盖、世界版本升级、完整E09/E10、真实模型/收益/发行。
历史first事实：回滚参考实际父212c，保留已有世界/报告/费用/撤销事实。无admin使用。AG071精确公共push两次自动审批拒绝和待用户授权仍挂起；AG074依主控要求从未请求外发，不能归因其本身工具拒绝。若main前进，必须重新叠放并全新独立验收后才可合并。验收结束已回收停止的纯target，所有QA/WT/branch/真实旧报告保持。
**历史first阶段（Codex原记录L784–793；原时点、原head与后来的公开/验收head分列）：**
历史段标题：ROOT 收尾 J46 — AG074 / PR127 实际合并，2026-10-11 UTC
历史first事实：- 用户最新要求：离开后按第一真源裁决完成无遗漏无尾巴收尾；完成时给简短交付指南。只处理现有交付，不新增产品任务、任务编号、子代理或派工。定时 rsiagent-30 已取消，不重建。
历史first事实：- AG074 四 Rust 文件保持原 d072619a 交付字节不变，在 main ea02f116 上固定提交 313598036461304fe1b081e91115bdbd2c779752，树 16de199ce2d66f4909b094603fd8def8207abb71。ROOT fresh14 全部真实 native0：8 个标准门槛、4 个正常 API 编译、head 两个完整二进制中的全部3个原独立反例。工作区 Rust116组1521通过，Python141含47 bare MCP全量通过，无失败/忽略/过滤。当前 parent 仅编译，不冒充新的 semantic red；旧 parent 失败仅作历史证据。SQL无写由未改动反例对完整无损逻辑快照的实际断言成立，不虚构本次持久化 ROOT catalog文件。非致命 rust-objcopy/libLLVM剥离警告保留原RAW。
历史first事实：- 专属分支公共推送、draft PR127创建、ready及 admin固定HEAD合并均实际成功，合并时间 2026-10-11T01:51:47Z。实际main aa51b1102e1886cbaf7b025d37b98ca30b6e2d3d，两个parent精确ea02/3135980，合并树精确等于验收树。ROOT原checkout仍618且clean；fetch仅更新origin/main。PR: https://github.com/acosmi/RSIAgent/pull/127 。第四轮 #98–127 共30项实际合并。
历史first事实：- 收尾后仅清理已终止074三个指定target的可再生非执行构建文件：实际unlink7878 REG，删除1395空DIR，保留337执行文件及所有Task固定执行文件完整字节/物理元数据。估计释放分配块2,005,139,456 bytes，清理后实际可用4,177,920,000 bytes。原源、QA/RAW/SQLite/环境/Git不清理。084合并后两target另已清理并保留247执行文件。低于10GB不是全局停止理由，按实际IO继续。
历史first事实：- AG076旧三文件无当前main重叠，旧Task及独立原head6/parent4 source已接收，本次只做必要整合复验。AG056 workflow scope现有token已恢复，仍需本次主线验收；不以旧作者1150/48计数接受现有主线。其它存量交付继续逐项处理。AG093冻结54处未提交修改及SourceSC仍保留，当前无新runtime、无Task接受、无发布。历史公共审批拒绝保留；不借其它通道绕过。
历史first事实：- 全部v4.2未完成，不标完成、不把support-scope219 needs_verification算作验收通过。产品手改=false，新增派工=false。原始结果及完整验收/清理凭据见本J46 JSON记录。
原未归属项的有限补述：AG074正常Deepen的实施与旧报告控制事实段。
原未归属项的有限补述：AG074旧core0/2、engine1/3正常编译；新core7/0、engine11/0，删门变异3/4与0/1；旧全量1286/5含5个HTTP EPERM，环境R1复跑1291/0与双smoke0分列。
原未归属项的有限补述：AG074卡ee6487be和环境R1 55dcc35a摘要保留；产品返修0、环境裁决1、旧测试零改。
原未归属项的有限补述：AG074只覆盖正常Deepen共享已观察父展开规则；不授一般PolicyStop/WorldExhausted、生产Recover、版本升级或完整E09/E10。
原未归属项的有限补述：AG074本地实施与旧报告兼容控制段，保原失败及环境裁决。
原未归属项的有限补述：AG074正常父0/2、1/3与新7/0、11/0分列；删门3/4、0/1，旧1286/5 HTTP EPERM与R1 1291/0分列，CLI旧readiness不能凭空称EPERM。
原未归属项的有限补述：AG074卡ee6487be、环境55dcc35a；产品返修0/环境裁决1/旧测试零改，记录原角色和时点。
原未归属项的有限补述：AG074仅正常Deepen的共享已观察父展开规则，complete_support/general stop/生产Recover/版本升级仍不宣。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E09/E10；E09.ag074_local_scoped_acceptance [withdrawn，旧通过不计当前，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG074-existing-mainea02-fixed3135980-ROOT-fresh14-runtime-r1-20261011.json`，文件SHA256 `9b4aa3a71d9c1e8123db3ac7e93615e4c50dcba8cf95c3405d369425d2f51f29`。
- 原卡导航：`out/audit-20260930/cards/AG-074.md`，文件SHA256 `ee6487be89271262896ef9f63869c23a25b46d86512260462a47838cad72a6f6`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG074-existing-currentEA02-fixed3135980-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `a3068eefde2f277daff51a6301ed30f31b0c09c61a53a2aef1b401b1c3971cd8`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `26841037c56a2816a5a6762b413374737ec23052c2b0aba8f6483a3612a58d31`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG074-existing-currentEA02-fixed3135980-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `caa26400fa38c55aadbb1828a5009bfe13cec0f3ad6e9e533eee89243a9ec1a7`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `26841037c56a2816a5a6762b413374737ec23052c2b0aba8f6483a3612a58d31`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG074-existing-currentEA02-fixed3135980-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/13/complete_stderr_UTF8`，原载荷SHA256 `8d03e47e5c55de24be12657f3680fb09262b31f85deed0b6ca708caacb263f02`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `26841037c56a2816a5a6762b413374737ec23052c2b0aba8f6483a3612a58d31`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG074-existing-currentEA02-fixed3135980-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/13/complete_stdout_UTF8`，原载荷SHA256 `46af6f4ec4cafa22c4d6e5021e0a1c592bd61a4e45357938e9758edfd1765714`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `26841037c56a2816a5a6762b413374737ec23052c2b0aba8f6483a3612a58d31`。
### PR #128 — AG-076（第四轮历史）
冻结M6角色：公开head 1355e95f598bf7d1f490d6d1d295a1e29c855c0c；本地验收head 1355e95f598bf7d1f490d6d1d295a1e29c855c0c；实际merge 1177c91931689794a7d28b614adc01d3a0435e46；mergedAt 2026-10-11T02:03:24Z。
合并父 aa51b1102e1886cbaf7b025d37b98ca30b6e2d3d, 1355e95f598bf7d1f490d6d1d295a1e29c855c0c；tree cd7b8f0db394eb2d5a417d34b5680d9edd27160a。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E16.1/verified_subscopes/0`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E16.1.ag076_local_scoped_acceptance；description = Bounded content-driven import ProgramFixture
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 1355e95f598bf7d1f490d6d1d295a1e29c855c0c；pr = https://github.com/acosmi/RSIAgent/pull/128；merged_sha = 1177c91931689794a7d28b614adc01d3a0435e46；pr_state = merged
历史原测试登记：test_entry = crates/evo-core/tests/import_fixture_v42.rs；command = cargo test --locked --offline -p evo-core --test import_fixture_v42；exit_code = 0；production_consumer = none_library_only
历史原证据登记：input_digest = 7c35c464e55005999b3d959c847edf8b90549d3290e4fce3e1882b424c106fd3；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-076-1355e95.json；log_path = out/audit-20260930/qa/takeover-20261002/AG076-existing-currentAA51-fixed1355e95-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E16.1; Bounded content-driven import ProgramFixture software/component boundary only. Library-only as of e727d1c: no production caller in engine/http/mcp/apps.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 1355e95f598bf7d1f490d6d1d295a1e29c855c0c; actual merge tree equals accepted tree cd7b8f0db394eb2d5a417d34b5680d9edd27160a. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #128; retain source, costs, revocation history and original QA. Accepted parent aa51b1102e1886cbaf7b025d37b98ca30b6e2d3d.
**历史first阶段（Codex原记录L429–465；原时点、原head与后来的公开/验收head分列）：**
历史段标题：CTRL-AG076-R1 / 本地独立验收（未建 PR、未合并） 2026-10-01T21:05:24.512154+00:00
历史first事实：固定 head `e706719af6b1b59a6b92329a2ea87fdd588d20a7`，实际父及当前origin/main `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `80cc17c2ff9d39d83b08305ba3832cc126cc5fe6`。唯一提交、无叠放/冲突，三白名单文件+2492/-0：875行新模块、lib一行、1616行新测试（2个旧API事实+32个新能力测试）。全部产品/测试已完整逐行预审，最终hash与冻结一致，lib仅fmt排序后的单行pub mod；工作树clean。无旧测试、依赖、锁或迁移改动。
历史段标题：独立复跑与反例
历史first事实：全新target-ctrl-ag076，`qa/ctrl-ag076-e706719.log*`八门实际0：fmt、clippy -D warnings、完整workspace、build rsia、双smoke、scope检查器、48 Python unittest。root逐行求和**1307/0/0ignored，97结果组=92测试bin+5doc**，等于父1273+34；jobs2只调资源，不过滤测试。
历史first事实：共同旧API source SHA `cc6c26bc1594f1194ee7e493f6e13d9eb9bc94bd6477bfec159245f6d1b179b1`，commonappend SHA `543a06606149f703316c1d1faa9df2582683dfa0f92d0f2160d2498b6b500124`。新API额外 SHA `11d339951e965b2d3afa9a5e8d2ad2d33a2ff70f2733c16e33f3dca9bf569883`，combined SHA `315dc505c182d3d58d80efdba2f0602e442fb6fed65156d46bd8c225246daa06`。
历史first事实：实际父 `ctrl-ag076-adversarial-base-212c784.log` 正常编译**2/0 exit0**；head `ctrl-ag076-adversarial-head-e706719.log` 正常编译**4/0 exit0**，两树事后clean。共同两例是边界核验，不能声称红转绿；新API缺失从未当编译失败证据。
历史first事实：- P1三个32KiB U+0001的part，各符合旧上限，literal98304而完整实际JSON590957字节仍被旧门接受，说明每part门不保证完整请求硬cap。原NUL夹具被旧text门合法拒绝，初始1/1日志及源码完整保留，不算产品缺陷；后来只改U+0001重跑。台账一处590948数字笔误已即时更正，原日志未改。
历史first事实：- P2旧编辑器allowed只是允许子集，能省略只反例/未选源。新fixture的required完整闭包另验，不悄改旧公共语义。
历史first事实：- Q3手算六种Abs/Deduplicate/Sort排列在非对称输入的实际输出，经完整编码、真实编辑编译、读取候选正文执行，再将候选JSON往返作新父二次生成no_change；六输入摘要不同、实际两案例不为空、所有包括count-only依赖保留，原Skill.dependencies不改；ProgramFixture与未验证历史轴分别保持。
历史first事实：- Q4在完整编码字节中注入unicode转义同名重复key（limits、method.program、parent），重新计算正确字节数与hash，仍固定fixture_invalid_request_json拒绝且不回显SECRET；原始合法请求的两案例和非ASCII父字段不变。
历史first事实：head探针运行前增强了cases.len==2/首轮正文实际改变，防止空for/all空过；旧版本源码分别留before-nonempty-assertions，实际父共同反例不受影响。
历史段标题：实施证据和边界裁决
历史first事实：实施最终报告 `out/audit-20260930/qa/impl-ag076/REPORT.md` SHA `eba495ba8f3f7b8bcf5a8e27e55eb344f4daf81a619950e38491332595d8defc` 已全文读；完整证据逐项实际hash/大小核对并归档，`ag076-reviewed-delivery.json`记录索引。基线先正常Cargo no-run0；资源不足原run98未执行；R1只准原已编译纯内存二进制实际2/0，明确不是Cargo实际run成功。新能力no-run0后34/0。
历史first事实：两变异正常编译后：固定oracle输出变异29/5、绕过实际输出预留变异33/1。第一次执行变异在测试进程尚未收尾时恢复源码，实施方如实披露并完整重跑严格顺序no-run0→29/5→session关闭→restore/cmp0，原首轮日志保留；最终三源码逐字相同。恢复后四bin58/0、clippy/build0。原full1302/5为五HTTP本地bind EPERM，R2同完整命令1307/0；CLI首次readiness1仅该事实，不冒称stderr EPERM，同命令/实际binary环境复跑0。整理计数时默认Python脚本exit1后改用既定runtime成功，原诊断留索引，不是测试失败，也未因此重跑测试。
历史first事实：卡 SHA `da488e2bf452dc52035be6ab697c1bca994906628a0c77862872bed08bad2e9f`；R1资源 `1d04d0ef83b9e354196d300398714c4b3ba8b36283c4f27428b60c58a1ad7c05`；R2监听环境 `957fece09cb7f0752a164899430452882a85329daf2331d4af54934d27b4300a`。无产品返修，两环境/资源裁决；原失败保留不筛。纯cache回收不删除源码/QA。
历史段标题：有限结论
历史first事实：本地verified仅E16.1/E02/E03可执行内存ProgramFixture：§5.3.1L495/499/503/507、§6.3L644/646/648/650、§6.4L658/664/684、§11.2L1100/1104、E16.1L1419–1423；V059/V089的完整编码与编辑边界、V076的内容因果前置切片。
历史first事实：可信调用方冻结control/P/B/全部202内材料引用；实际完整输入UTF8 JSON每字节一个本地fixture token，输入<=131072、输出<=65536、context<=196608，32案例/每例32整数/最多4封闭操作。实际同一字节序列解码并生成完整SkillEditBatch，再原解析/编译、保护区/4096限制。独立oracle不调用候选执行器；条件不满足Unsupported，0步identity/no_change不算学会。这里的token是固定字节词表，绝非真实模型估计；无真实provider调用、无伪造账单/应用/正式回执。
历史first事实：仍未接三reader、AG070材料读取、AG077预算、存储授权/撤销当前水位、持久Pattern/Candidate、正式门或新真实run；**完整V076/导入学习链仍未完成**，不能把本内核当整链验收。未知自然语言、真实tokenizer/模型/沙箱、正式批准、统计收益和发行不在可声明范围。
历史first事实：无PR、未push/merge、无admin，不计已合并产品/support-scope。AG071精确公共推送的自动审批拒绝与待人类授权仍挂起；本任务遵照主控未申请外发，没有自身外发拒绝。main变化须重新叠放并fresh独立验收。回滚参考父212c，既有账单/撤销/历史不逆转。
**历史first阶段（Codex原记录L794–804；原时点、原head与后来的公开/验收head分列）：**
历史段标题：ROOT 收尾 J47 — AG076 / PR128 实际合并，2026-10-11 UTC
历史first事实：- 按用户既有 admin 交付通过即合并授权和现有收尾范围执行，不新增产品任务、任务编号、子代理或派工；定时仍取消。用户要求完成后给简短交付指南，尚未到全部存量闭环，不先宣称完成。
历史first事实：- AG076 原707790ab三文件+2492/-0字节未改，在 main aa51b110 固定1355e95f598bf7d1f490d6d1d295a1e29c855c0c，树cd7b8f0db394eb2d5a417d34b5680d9edd27160a。ROOT fresh12全部真实native0：标准8门、head/parent正常API编译和各自完整6/4反例；Rust117组1555通过，Python141含47 bare MCP，全量无失败/忽略/过滤。parent4是原API正向边界，不冒充semantic red。ProgramFixture仅纯开发固定字节/资料绑定，不接受真实模型/可信生产host/import consumption/授权撤销/收益/正式效果或全E/V。support-scope219 needs_verification保持。
历史first事实：- 初始只读preflight因新计划schema标识写错而在启动marker及任何runtime之前失败；原card保留，r2只修正协议标识、保存原失败并重新核对所有源全物理保护，实际preflight通过后才启动一次fresh12。没有代码改动或验证并发重开。
历史first事实：- PR128专属分支推送、draft创建/ready完成。第一次admin GraphQL请求返回EOF/native1；随后实际API仍mainAA51、PR128 OPEN/未合并，才对同一固定head和body重试。重试实际native0；PR128在2026-10-11T02:03:24Z合并。实际main1177c91931689794a7d28b614adc01d3a0435e46，parents精确AA51/1355，树精确等于验收树。PR https://github.com/acosmi/RSIAgent/pull/128 。第四轮#98–128共31项实际合并。
历史first事实：- 076结束并合并后仅清理三个明确target的可再生非执行构建文件：unlink5026 REG、907空DIR，保留242执行文件及所有Task固定执行文件的字节和元数据；估计分配块1,010,769,920 bytes，清理后可用3,936,256,000 bytes。ROOT旧checkout/source/RAW/QA/SQLite/Git/环境不清理；fetch仅更新origin/main。低于10GB不作为全局停止原因。
历史first事实：- AG056旧CI文件4858bytes/SHA dbb2c775保持，仅原+52/-2 workflow增量在main1177整合，固定e991c86待本地复验。原workflow OAuth scope阻塞已解除。已准备现有任务的私有独立QA Source r2，直接执行冻结真实CI块：33 gate非success反例、4 checksum变异/缺失反例、inventory/archive原生退出传播、真实statistics_reference/真实inventory。不会派发Actions或宣称Linux/runner；inventory保留语义gap和实际stderr/stdout两RAW拼接不代表跨流顺序的边界。原baseline和作者结果保留历史身份。SOURCE准备期两次解释器/生成文本语法错误均发生在主验证或产品写入前，不用作验收。
历史first事实：- 其余存量070/075/086/087/088/089/090/091/092/093继续逐项；093仍54处冻结未提交修改/SourceSC接收，没有新runtime/Task/发布。公共拒绝历史保留，不走不同通道绕过；不把未验证或未合并登记为交付。整个v4.2持续目标未完成。
高风险历史 DISK_ARCHIVE_AG076：L8500的076首次gzip归档ENOSPC/native1是维护失败，原程序未删，r2完成；不可并入AG070 parent/head或AG093验证失败计数。
原未归属项的有限补述：AG076 ProgramFixture因果前置切片的有限结论段。
原未归属项的有限补述：AG076仅验证可执行内存ProgramFixture的完整编码和编辑边界；V076只因果前置，不是完整学习链。
原未归属项的有限补述：AG076冻结完整202材料；UTF8每字节一个fixture token，输入131072/输出65536/context196608、32案例/32整数/4操作；真实编译并执行，独立oracle，不支持或0步identity不冒学习收益。
原未归属项的有限补述：AG076 ProgramFixture内存执行有限结论段，区别真实模型执行。
原未归属项的有限补述：AG076仅完整编码/编辑边界与V076因果前置，未完成整个导入学习链。
原未归属项的有限补述：AG076完整202材料、131072/65536/196608字节token合同、32案例/32整数/4操作及独立oracle真实执行；0步或不支持不算学习。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E16.1；E16.1.ag076_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG076-existing-mainaa51-fixed1355e95-ROOT-fresh12-runtime-r2-20261011.json`，文件SHA256 `8bc491b072d51a93dc69b46daa1cf8b3146b1faf0dddd5651ee9680f6ec97c52`。
- 原卡导航：`out/audit-20260930/cards/AG-076.md`，文件SHA256 `da488e2bf452dc52035be6ab697c1bca994906628a0c77862872bed08bad2e9f`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG076-existing-currentAA51-fixed1355e95-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `fc9fdd262d4dd2779817c29d06d4bb3b717868d28d6e29cf8daff001811aa04c`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `1de069f7b3e78d9d34b27ba0f145367c7d391351bad7d787db8698e727b21e89`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG076-existing-currentAA51-fixed1355e95-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `5e69b8f8d37cc3e911713850dfd7be58e8cc83313104192c97bc737f5a97740e`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `1de069f7b3e78d9d34b27ba0f145367c7d391351bad7d787db8698e727b21e89`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG076-existing-currentAA51-fixed1355e95-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/11/complete_stderr_UTF8`，原载荷SHA256 `4e6d5e0a74a4e7e900fa558e2a803f2904b49607392c7c856acac51d9fe32af0`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `1de069f7b3e78d9d34b27ba0f145367c7d391351bad7d787db8698e727b21e89`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG076-existing-currentAA51-fixed1355e95-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/11/complete_stdout_UTF8`，原载荷SHA256 `05846be8fdfb88e2f49815fd27a0d6265705289f27c2a14a9c37add571118d2b`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `1de069f7b3e78d9d34b27ba0f145367c7d391351bad7d787db8698e727b21e89`。
旧v4.2阶段 B06（H `707790ab6bb00174bd4bd1d9528aae9242a0cdab`；实际Parent `06fb166b006e5479a80e04dc14998109c30b0d42`；原时点，本地/失败/授权角色与本节最终头分列）：
历史阶段补述：旧7077/Parent06fb完整14/14实际case全0：workspace1397/103groups、Python48；普通no-run分别先于directed34、Parent whole4、Head whole6，各whole零其他不通过项且不重复相加workspace。Parent old-API4全绿是父边界，不能假称missing-new-API语义红；不借后1355阶段1555。
历史阶段补述：初case02在真实Clippy前被Root grant shadowing拒98，原件保；纠正精确grant后Clippy实际只一次。旧PASS_LOCAL_TASK只释放AG076本地epoch，MAIN=null，必要SC0和AST推导Python名字/实际非verbosecount区别保；局部ProgramFixture不证实际Provider/付费收益/完整E14/E15。
精确旧阶段原像导航：`historical-original-ledger` 的 ledger:5448；ledger:5476；各完整原LF SHA及文字映在本地R11覆盖；本补述不授新验收。
### PR #129 — AG-056（第四轮历史）
冻结M6角色：公开head e991c86eafb40e2bccbd8fe2d1aa3c184b9d92bb；本地验收head e991c86eafb40e2bccbd8fe2d1aa3c184b9d92bb；实际merge 3ad643ca2a1cf57f4e7bc1c0f032eef2663902b5；mergedAt 2026-10-11T02:22:51Z。
合并父 1177c91931689794a7d28b614adc01d3a0435e46, e991c86eafb40e2bccbd8fe2d1aa3c184b9d92bb；tree 7ec8d9352baf83a67000114336dbcb3f0737e650。原双父合并角色；不是本轮网络重查或重新执行。
**历史first阶段（Codex原记录L805–815；原时点、原head与后来的公开/验收head分列）：**
历史段标题：ROOT 收尾 J48 — AG056 / PR129 实际合并；已有前端重新纳入存量，2026-10-11 UTC
历史first事实：- 遵照用户既有 admin 验收通过即合并授权继续现有交付闭环，不新增产品任务、子代理或派工，定时仍取消。完成后给简短交付指南；当前尚有存量，不先宣称收尾或v4.2完成。
历史first事实：- AG056 原19013959单workflow4858bytes/SHA dbb2c775、+52/-2未改，main1177固定e991c86eafb40e2bccbd8fe2d1aa3c184b9d92bb/树7ec8d9352baf83a67000114336dbcb3f0737e650。fresh10真实全native0：标准8门、独立正常编译和whole2；Rust117组1555通过、Python141含47 bare MCP，无失败/忽略/过滤。源482REG/93DIR、index等全物理保护精确。
历史first事实：- ROOT物理读取全部原始result/invocation/stdout/stderr、112实际工作区Running执行文件和385实际CI反例artifact。字面CI块33非success gate反例、4校验和变异/缺失反例正确native1；inventory/archive注入失败保持native2/真实诊断，正常inventory/statistics/archive0。真实inventory记录1539、初始3passed/1536unverified、1555原始log观察；test_log_source_attested=false、legacy_equivalence_unverified=true，不把观察升格源验收。实际完整stderr加换行stdout只是输入拼接，不代表跨流时间顺序。统计2000次为program_check_not_product_effect。未运行Actions/Linux，不接受真实模型/收益/全v4.2效果；support-scope219 needs_verification保持。
历史first事实：- PR129实际推送、draft/ready及admin合并完成，2026-10-11T02:22:51Z；真实main3ad643ca2a1cf57f4e7bc1c0f032eef2663902b5，parents1177/e991，树精确等于验收树。PR https://github.com/acosmi/RSIAgent/pull/129 。第四轮#98–129共32项真实合并。ROOT旧618干净checkout保留，fetch只更新origin/main。
历史first事实：- 056两已结束target只清可再生非执行文件：unlink4675REG/826空DIR、估计分配块931102720bytes，215执行文件及全部Task固定执行文件保护原字节/full14，清理后可用3362816000bytes。Source/RAW/QA/SQLite/Git/环境不清。低于10GB不全局停止。补充只读发现时一次诊断文件路径猜测错误/native1已原样存档，无产品改动或第二验证。
历史first事实：- 前端专用窗口“等待后端排除任务”已有交付也在本次收尾范围：独立clone frontend-v3-status-character-20261006，固定a0ea253bc3971783e64008f57da977652b017d50，14个frontend/status-character新文件；用户已批准透明3D状态小人且明确不是宠物，原包只作设计依据。最新修正恢复右上状态胶囊、去掉重复底部文案，旧本地Node10、WebGL mock12、真实受阻/断连证据存在；仅旧主线局部ROOT接收，当前main整合RuntimeSC/Task/PR仍null。没有像素1:1原生验收/全状态真实pipeline，不将旧前端证据重贴为当前通过。通用r3 recorder没有前端task白名单，不伪装AG编号或改冻结脚本绕批准；后续只按已有前端范围做独立验收/字面整合，无新派工。
历史first事实：- 后端余存量070/075/086/087/088/089/090/091/092/093；另有上述已有前端。09354处冻结未提交修改、SourceSC已接收，RuntimeSC/Task/发布仍null。旧具体公共拒绝保留，后续实际审查决定，不换通道绕过，不把待验收/待合并算已交付。
高风险历史 P129_AUTH_SCOPE：J48沿既有admin条件授权收尾；旧workflow OAuth scope恢复与旧公共拒绝在原历程保留，不把token恢复当当时主线新测试结果。实际fixed e991/P1177。
高风险历史 P129_TECH_FAILURE_BOUNDS：J48 fresh10/Rust1555/117/Python141只是本地实际记录；33非success、4checksum变异期望1，inventory/archive注入期望2与正常0并列，是反例成功而不是无失败。
高风险历史 P129_NOT_CI_MODEL_EFFECT：inventory1539(3passed+1536unverified)/1555 log观察、test_log_source_attested=false、legacy=true；stderr+stdout拼接不证跨流顺序，统计2000仅program_check；无Actions/Linux/provider效果/全v4.2。
高风险历史 P129_LATER_CI_DEFECT：Claude二验另定位CI unittest discover将47 MCP裸用例纳入而缺CLI初始化，实际CI构造可红；不让旧本地fresh141掩盖，后续AG098修复记录保原protected段。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E00；未入当前98单目标索引：CI工作流。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG056-existing-main1177-fixede991c86-ROOT-fresh10-runtime-r1-20261011.json`，文件SHA256 `0e3dfe2a749be504ecf8d5e006822e3dbe510fa5015d70687c04ec4c60002f27`。
- 原卡导航：`out/audit-20260930/cards/AG-056.md`，文件SHA256 `a996d6f23750c37f2d4af6b55a6fa0521c7f7467714ce11b878d144987ddb14d`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG056-existing-current1177-fixede991c86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `acd429a2292eaf5b2e0703667407b19504a6960d827f64d6972e6a98d2b0e8d4`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `c0192641489270e27fd28ece967d53d9b3127ed646bc35333da57c710183e0d2`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG056-existing-current1177-fixede991c86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `b66d7a2dc36c98d5bceb300daf7f6a3e6b192d9d340ff92a18cba31b400991ae`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `c0192641489270e27fd28ece967d53d9b3127ed646bc35333da57c710183e0d2`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG056-existing-current1177-fixede991c86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stderr_UTF8`，原载荷SHA256 `2066dc6dc3d57d91f45c73554b68587987d1892e63682b069fd538db3153da35`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `c0192641489270e27fd28ece967d53d9b3127ed646bc35333da57c710183e0d2`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG056-existing-current1177-fixede991c86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stdout_UTF8`，原载荷SHA256 `e5f855afbe76603adb882a9a84a143c8fef450ac649812e11381b459535e902c`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `c0192641489270e27fd28ece967d53d9b3127ed646bc35333da57c710183e0d2`。
### PR #130 — AG-070（第四轮历史）
冻结M6角色：公开head 887d97a345e947c754dd01c0fd0e1dd15b635d5c；本地验收head 887d97a345e947c754dd01c0fd0e1dd15b635d5c；实际merge 1132b6b4b6e0016540339ba65219daf1ceb35fb8；mergedAt 2026-10-11T03:39:33Z。
合并父 3ad643ca2a1cf57f4e7bc1c0f032eef2663902b5, 887d97a345e947c754dd01c0fd0e1dd15b635d5c；tree 40a7da9bf227bfa26b8d44bf6ce49d19037986aa。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E16.1/verified_subscopes/1`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E16.1.ag070_local_scoped_acceptance；description = Bounded local development-material reader
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 887d97a345e947c754dd01c0fd0e1dd15b635d5c；pr = https://github.com/acosmi/RSIAgent/pull/130；merged_sha = 1132b6b4b6e0016540339ba65219daf1ceb35fb8；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/import_development_material_v42.rs；command = cargo test --locked --offline -p evo-engine --test import_development_material_v42；exit_code = 0；production_consumer = none_library_only
历史原证据登记：input_digest = b22d74557ec1e8d8ba3627921a49cf165cfb3c6e7477b411e1bc94d44221d78e；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-070-887d97a.json；log_path = out/audit-20260930/qa/takeover-20261002/AG070-existing-current3ad-fixed887d97a-ROOT-standard8-head38-parent36-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E16.1; Bounded local development-material reader software/component boundary only. Library-only as of e727d1c: no production caller in engine/http/mcp/apps.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 887d97a345e947c754dd01c0fd0e1dd15b635d5c; actual merge tree equals accepted tree 40a7da9bf227bfa26b8d44bf6ce49d19037986aa. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #130; retain source, costs, revocation history and original QA. Accepted parent 3ad643ca2a1cf57f4e7bc1c0f032eef2663902b5.
**历史first阶段（Codex原记录L242–266；原时点、原head与后来的公开/验收head分列）：**
历史段标题：未合并附录：AG070 本地独立验收完成
历史first事实：固定head ac90c02bca65a4630ebc51f5518dc406c7038599，实际父212c78488b403397eaebaa65a6d280b9c6be3899，tree36781065c859ccdd95f1afff331db68697e6c09b。无PR、未push/ready/merge，不计已合并任务。
历史first事实：原b895934ce1d18c64766d26a958661bdaf717b573从200c43bc19368ff52b064e67eb1b14e35f096d39本地重基至当前main212c784；唯一提交前后patch逐字节cmp0，无冲突。未有远端分支，无force-with-lease动作。四白名单文件+1971/-2完整逐行预审，最终hash与预审一致；三个原有产品块及原测试逐字保持，仅新增本地消费者/合同/私有三测试及1018行集成12测试。交付/探针后WT clean。
历史first事实：全新target-ctrl-ag070，ctrl-ag070-ac90c02.log*八项exit0：fmt/clippy/workspace/build/双smoke/support checker/unittest；root独立求和97结果行=92测试二进制+5doc组，1288/0=实际父1273+15（12集成+3返回边界），Python48。jobs2只限制资源，不过滤测试。
历史first事实：root冻结source-import-v41-212c784.rs SHA c6d4f537a0323a67e7cc40753f0d94682fb360db20532a7c9d3539de30373dd2；共同append.rs SHA2abd0ca8c480973dd40ec93d282a46ca5cb8c9ea7fb687de0669492dcd92324f。
历史first事实：- P1两个来源完整source_refs换序且重算input_digest，旧公共load_live_result重复请求仍精确拒绝source mismatch，artifact/edges/auditcount不变。
历史first事实：- P2未选第二来源的受控blob换成内容完全相同的symlink，读取主来源fragment仍拒绝且DB/canary不变；unlink还原原文件后正向再读成功。
历史first事实：- 实际父ctrl-ag070-adversarial-base-212c784.log两项2/0 exit0；head共同两项也2/0。两树同一冻结源码/append均正常编译、事后clean。属于边界核验，不冒称旧产品缺陷红灯。
历史first事实：- 新API额外Q1重绑全部请求摘要后新consumer仍拒换序闭包；Q2单metadata的30000个NUL经JSON转义已超过128KiB，原raw片段仍可读，新材料返回零摘录/实际序列化数组2字节/partial与cursor0/全部来源保留，DB不变。
历史first事实：- head-new-api.rs SHA c9267fbcda7a708dab61712f9a89877170ef6eec5cc0e27c2ae4edcd39dbdabe；共同与额外拼接append-with-head-extras.rs SHA cb7b2f1486aeee844d1f75855cd5ba406cb11f0b880ceb2390a677037c022f49。ctrl-ag070-adversarial-head-ac90c02.log共4/0 exit0。旧树没有新接口，两额外项只验head，不用缺符号编译错假造基线。
历史first事实：实施报告qa/impl-ag070/REPORT.md SHA16062df0091f9a22681e81342d302597f8f667d30c719433a7ff13bbca99a7bb已完整读。其旧基线公共API真实2/0；新增首错9/2为SQLite id CHECK夹具，纠正后12/0。删重算0/1、删返回前DB复核0/3均正常编译后101，精确恢复；最终hardlink断言已真跑12/0及两次full。初full1273/5为五个旧HTTP EPERM，R2同命令完整1278/0（该树父200c1263+15）；初CLI只readiness1未打印stderr，不单独称EPERM，同命令环境0。所有原失败/工具设置失败保留，非产品返修。
历史first事实：verified仅E16.1独立导入材料读取段：§6.3L629/L644/L646/L648/L650、§6.4L658/L684、§11L1077/L1110/L1132–1134、E16.1L1419–1423；V005/V006/V017/V051–56/V076限定已实现三固定reader register→execute→真实typed本地consumer与来源活性。原owner Admin、Development、全result/selection/来源对象fingerprint、注册授权与reader/派生内容重核；未选/聚合/不可读源都保留闭包；实际blob一次读取及共享上游撤销门；返回前重载全DB对象。固定ImportedHistory/UnverifiedImport/Development、Unreviewed/NoModelDispatch，不改变旧G1。
历史first事实：边界：64MiB读取预算对每Ready源保守多计1探测字节，不能完整重核则明确拒；摘录数组≤128KiB/≤32，coverage与原import分开。内容仍可含敏感值/命令/答案，只限本地域，未调用scan当安全证明。R1明确固定Vec是字节快照，不能证明复制后无DB变化的外部同名文件永远不变；未来consumer必须重新读取并核授权/撤销。cursor仅信息、不支持offset重入，core validate只是形状/摘要校验不是存储权威。返回不是长期授权；事务快照之后发生的撤销不作跨时线性化保证。
历史first事实：不可声明安全出站、持久Pattern/RoutingDecision、E16生成/typed Broker预算/真实Skill消费runner/Candidate/formal/new-run、完整V076或完整默认来源支持、真实模型/收益/发行。完整学习链仍必须继续实施，不因本段通过标完成。回滚参考实际父212c784，仅移除新增API/类型，不逆转已发生费用/撤销/历史或恢复明文。
历史first事实：卡2dbd6904f5271a4a1d5c8ea16c27398afac8f7e56d7241a8ebff07b0e35e9127；R1字节快照93acd38dbd2cbebc376735a837346dae909470b0ce89932aeb3fd47067ef484f；R2环境b3dbfbc55c9f73d5062a7bb16f3fa675a95560770d502dcfcf84da57cbde975f。产品返修0，合同纠正1、环境裁决1，无冲突/admin未用。AG070自身未遭外发工具拒绝，因为遵照root统一暂停没有尝试push；AG071精确公共外发双重拒绝待用户回复仍有效。后续若main前进，必须重基并全新独立验收再合并。
**历史first阶段（Codex原记录L816–826；原时点、原head与后来的公开/验收head分列）：**
历史段标题：ROOT 收尾 J49 — AG070 / PR130 实际合并，IO恢复与旧已合并target无损归档，2026-10-11 UTC
历史first事实：- 保持现有收尾范围，无新产品任务/编号/子代理/派工，定时取消，完成后交付简短指南；整个v4.2目标未完成，不提前收尾。
历史first事实：- AG070 原de24四文件+1971/-2在main3ad字节未改，固定887d97a345e947c754dd01c0fd0e1dd15b635d5c，验收树40a7da9bf227bfa26b8d44bf6ce49d19037986aa。原完整标准8真实0：Rust118组1570 /Python141含47 bare MCP，无失败/忽略/过滤；新隔离counter仍用原38/36全套源，正常API编译后各自whole全部0，父版本是原API正向保护边界，不冒充semantic red。
历史first事实：- 第一次主验证仅反例head正常编译因真实ENOSPC/native101退出；标准8通过原记录和源完整保留。独立四反例有限恢复使用新独立target、明确opt-level1，head正常编译和whole38通过，parent正常编译第二次真实ENOSPC/native101退出。释放结束缓存、归档已合并程序后，仅在新的独立parent target恢复正常编译+whole36，真实均0。原master两次101、原完整RAW/invocation、所有源full14/索引保护保留，不把失败进程改记0，不重跑已通过的冻结同head八门或head38，不修改任何产品字节。两有限恢复记录器各新固定SOURCE/新批准前缀，原r3/r1脚本不改，不复用前缀绕审批。
历史first事实：- 私有ROOT Task已接收12项实际通过结果（8原标准门+head正常/whole38+parent正常/whole36），固定同HEAD/parent/tree；正常与whole程序字节/full14一致；113实际工作区Running程序。最终可审正文1889bytes/SHA3107b489。快照返回值不保证外部文件在读后不变，后续使用仍要新验证binding/liveness/外发授权；ImportedHistory/UnverifiedImport/Development、Unreviewed/NoModelDispatch不提升。非全学习链/路由/BrokerRunner/物理擦除/真实模型host/效果/发布验收；support-scope219 needs_verification保持。
历史first事实：- 遵照用户后来“交付通过的admin批准合并，不要再问我授权”，在完整当前主线验收和原四文件不变后经原Git push通道实际获准推送。原20261003具体公共拒绝留存，未换通道规避；本次新实际推送、draft130/ready、admin固定887合并成功。PR https://github.com/acosmi/RSIAgent/pull/130 ，mergedAt2026-10-11T03:39:33Z。真实main1132b6b4b6e0016540339ba65219daf1ceb35fb8、parents3ad/887、树等于验收树。第四轮#98–130共33项实际合并；fetch仅更新origin/main，ROOT旧618 checkout保留。
历史first事实：- 070结束后六明确target可再生非执行缓存清理真实2118REG/386空DIR、估计548605952bytes、400执行文件与全部Task程序保持原字节/full14，清理后6717440000bytes可用。此前070标准8结束后释放4336REG/850718720bytes，实际ps受sandbox拒绝后按规则同命令升级获准，原失败/native1留存；独立counter在其他target不受扰。
历史first事实：- 磁盘确有IO满盘而非10GB门槛停止。依启动提示词第3.5节“删除…实施方与验收的target”及第6节“删除已验收任务的target（不删正在使用的）”，补做已合并056/074/076/084旧fresh8程序无损gzip归档与target退役：每文件原字节/全metadata先核对，压缩文件fsync/只读，完整解压逐字节/SHA相同并先写独立不可变checkpoint，才unlink原执行文件，保存逐项原full14→archive full14映射及闭合收据。分别188/187/188/185程序，估计净释放1635553280/1632530432/1635553280/1618358272bytes。所有原Task/RAW/QA/source/DB保持原文件；历史原程序路径与metadata此后为退役历史，不能再声称旧执行文件仍在原路径，可从受验archive恢复原字节。当前070物理guard范围逐一证明与这些旧target无交叠且前后精确。076首次归档因真实ENOSPC失败，未删原程序，旧不完整gzip/原begin及native1留存；r2在释放已结束parent失败缓存后完成，未覆盖旧失败。Docker可回收缓存实查归属其他项目，没有清理。
历史first事实：- 余存量后端075/086/087/088/089/090/091/092/093，加已授权前端14文件a0ea。前端仍仅旧主线局部接收，当前整合RuntimeSC/Task/PR空，明确3D状态小人非宠物；09354处冻结未提交修改/SourceSC已接收、RuntimeSC/Task/PR仍空。既有未通过/未合并及实际阻塞不算交付，不留虚假完成状态。
高风险历史 DISK_AG070_HEAD_PARENT：L8496的“两次ENOSPC”是AG070验证head普通编译101与parent普通编译101两个独立角色；standard8和head38不能重标为新重跑，parent36恢复0另列。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E03/E16.1；E16.1.ag070_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG-070.md`，文件SHA256 `2dbd6904f5271a4a1d5c8ea16c27398afac8f7e56d7241a8ebff07b0e35e9127`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG070-existing-current3ad-fixed887d97a-ROOT-standard8-head38-parent36-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_twelve_accepted_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `64063eed67a7e110046d8450e9ca1acf9f478dad973a69cf83ef1fd04178ecaf`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `aaf45b6c5f63f4d1f50cc39864cc7ac00b31c1b94dad25d5a1888e7d68ecc8c8`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG070-existing-current3ad-fixed887d97a-ROOT-standard8-head38-parent36-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_twelve_accepted_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `7add4aa4c92b09e5f675a855690f5accd1f590bc10d684f681492c838383171d`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `aaf45b6c5f63f4d1f50cc39864cc7ac00b31c1b94dad25d5a1888e7d68ecc8c8`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG070-existing-current3ad-fixed887d97a-ROOT-standard8-head38-parent36-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_twelve_accepted_actual_cases/11/complete_stderr_UTF8`，原载荷SHA256 `ae17504978d451cc1c5a64979cc45416371c5b4030ac53d9d723d22917b97f45`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `aaf45b6c5f63f4d1f50cc39864cc7ac00b31c1b94dad25d5a1888e7d68ecc8c8`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG070-existing-current3ad-fixed887d97a-ROOT-standard8-head38-parent36-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_twelve_accepted_actual_cases/11/complete_stdout_UTF8`，原载荷SHA256 `480de087deb56ad664289e2e980385558749dd0ac6ce6e17eb406d3a7408b04b`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `aaf45b6c5f63f4d1f50cc39864cc7ac00b31c1b94dad25d5a1888e7d68ecc8c8`。
### PR #131 — AG-075（第四轮历史）
冻结M6角色：公开head 8a92e578527b6334b5fb776abf46c5bc9a88e4b7；本地验收head 8a92e578527b6334b5fb776abf46c5bc9a88e4b7；实际merge aa831fba2d09d0ef2d0755c3d7a08971b8f6667f；mergedAt 2026-10-11T04:00:06Z。
合并父 1132b6b4b6e0016540339ba65219daf1ceb35fb8, 8a92e578527b6334b5fb776abf46c5bc9a88e4b7；tree 2cfe492ce726c7ce1e44ca0cc9736c857b2bc072。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E08/verified_subscopes/10`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E08.ag075_local_scoped_acceptance；description = Direct typed import source/result dependency binding
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 8a92e578527b6334b5fb776abf46c5bc9a88e4b7；pr = https://github.com/acosmi/RSIAgent/pull/131；merged_sha = aa831fba2d09d0ef2d0755c3d7a08971b8f6667f；pr_state = merged
历史原测试登记：test_entry = crates/evo-storage/tests/dependency_read_v42.rs；command = cargo test --locked --offline -p evo-storage --test dependency_read_v42；exit_code = 0；production_consumer = none_library_only
历史原证据登记：input_digest = 7d940277f3f534775adfb8c60054927ed70b84277d9707fff4cd55fab9744141；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-075-8a92e57.json；log_path = out/audit-20260930/qa/takeover-20261002/AG075-existing-current1132-fixed8a92e57-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E08; Direct typed import source/result dependency binding software/component boundary only. Library-only as of e727d1c: no production caller in engine/http/mcp/apps.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 8a92e578527b6334b5fb776abf46c5bc9a88e4b7; actual merge tree equals accepted tree 2cfe492ce726c7ce1e44ca0cc9736c857b2bc072. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #131; retain source, costs, revocation history and original QA. Accepted parent 1132b6b4b6e0016540339ba65219daf1ceb35fb8.
**历史first阶段（Codex原记录L395–428；原时点、原head与后来的公开/验收head分列）：**
历史段标题：CTRL-AG075-R1 / 本地独立验收（未建PR、未合并） 2026-10-01T20:27:10.779214+00:00
历史first事实：固定 head `3d4995a4c0e122a409908aca21c512fc46372dce`，实际父 `212c78488b403397eaebaa65a6d280b9c6be3899`=当前origin/main，tree `3da9ea4f747931c2881a4288b06eba86ded80402`。无叠放；六白名单文件+2860/-0，两lib仅pub mod，旧测试/依赖/锁/迁移未改。主控此前完整逐行读全部产品与新测试；最终六hash与预审冻结及交付manifest一致，工作树clean。
历史段标题：独立复跑与反例
历史first事实：全新target-ctrl-ag075，`qa/ctrl-ag075-3d4995a.log*`八门实际exit0：fmt、clippy、完整workspace、build rsia、双smoke、scope检查器、Python unittest48。独立逐行求和98组=93测试bin+5doc，1301 passed / 0 failed / 0 ignored，等于父1273+28新增。jobs2仅资源调度，未过滤测试。
历史first事实：冻结旧API源 `qa/probe-ag075/source-old-api-212c784.rs` SHA97d847304788bd3abece824d69853edc8c07f26313ca7377061208fc8de3faa1，共同append SHA3132fc075513f7afd2db58074aef7faec08a0b27f28e8c4733bd9aff4a9517c7；新APIhead额外 SHA207d1c04ec489e0c30fd35dd215f9d2a8c94e3415c3ae26ad8e02f9050ef9e83，合并append-head SHAc5ad3ff95eece26d50400b451a3a5dc2f60c84f02c48091904b9c23f2452c202。
历史first事实：实际父 `ctrl-ag075-adversarial-base-212c784.log` 正常编译2/0exit0；head `ctrl-ag075-adversarial-head-3d4995a.log` 正常编译4/0exit0；两树探针后clean。共同两例是诚实边界核验，并非红转绿；新API只在head存在，缺符号编译错误没有用于基线证据。
历史first事实：- P1真实writer产物复制后缺result→非primary直接边；传递路径仍有它，foreign namespace同名边和300额外边不能补齐自身缺边；旧load成功事实不变。
历史first事实：- P2仅第二source换成foreign owner，旧result load拒绝且primary原样；全SQLite规范快照前后相同。
历史first事实：- Q1 page_size=1且行预算0/1/2/3/256/257均Partial，不在未穷尽时猜缺边，lookahead实际读取也不超过预算；预算10000、页宽1/17/256均穷尽302边，精确定位1条缺非primary边与300条额外unknown边，不让foreign同名边掩盖问题；报告physical_blob_check=not_checked。
历史first事实：- Q2 source-only仅读自己及selection backlink两对象，不读取/背书foreign sibling，selection不展开边；result/selection完整检查必须Forbidden。两类head诊断全部SQLite表前后相同。
历史段标题：实施证据及裁决
历史first事实：交付 `qa/impl-ag075/final-report.md` SHAce116045ffa15d586d93ceaf5df503908f4a4c10effebfe358e0dc827bb54909，manifest04338dcfd397d561d08c0cdf129bdc88bdca71cf363d76c20596df27d2555913，79项evidence-index ffe84d10d9f5509990d8422495549de78769c88c80e5c34ab578ab86a7a28185已逐项实际大小/hash归档；报告全文和28项命令表已读。
历史first事实：自测旧API no-run0、真实两例2/0，最终storage4+engine24=28/0；省非primary边变异最终正常编译后18/6、去namespace变异3/1，恢复cmp0。原FTS rowid夹具错、Option编译错、两clippy lint均留原日志，不算产品红灯。原workspace1296/5五个HTTP bind EPERM，R2同完整命令1301/0；原CLI只证readiness超时exit1，不冒称该文本证明stderr EPERM，同cmd同binary环境复跑0。主控此次独立八门全过。
历史first事实：卡SHAf3a6621e090f0a04810b9a2f63f61e2179bb325e02852237e24611a237befe46；R1 532744f534d6763b0741a58a22110a766cbc68e07ec2b2f42d67dedf1cd90a36，R2 d5fafcbe2c826ab4875b04166e82f2f8dca72d771ebf401f70b6adac1b4c0458。R1坏JSON口径以正常SQLite CHECK为准，合法JSON typed坏字段分类，底层SQL/JSON失败原Err；R2仅监听环境复跑。未要求产品返修；实施期clippy整改属原白名单。
历史段标题：结论与未完成范围
历史first事实：本地verified仅E08有界typed E16声明/直接边只读诊断：E08L1295/1299、§6.3L644/648/650、§11L1077/1081/1110/1112/1132–1134，V017/V018/V056/V075有限诊断切片，V076仅前置。一个Session快照、最多202对象点读/10000实际边行/256页宽；不穷尽必Partial。已知typed fingerprint与Value重排分开；合法prepared/failed/进行中摘要转变保留Unknown/合法状态；redacted仅History，额外边Unknown，不能当已证明污染。
历史first事实：不可声明全部namespace/所有E08对象完整性、业务授权/当前live消费门、物理blob/文件检查、绝对正文内存CPU上界、数据库修复、完整三reader学习新使用撤销链、真实模型/收益/发行。source-only的selection只是归属回指权威，不扩大为其全部兄弟闭包。namespace水位是诊断快照，不能当返回时刻授权。没有CLI/自动修复/持久化报告接线。
历史first事实：未建PR、未push/merge、无admin，不计合并产品或support-scope已合并记录。AG071精确公共push自动审批拒绝及待人类授权仍挂起；AG075自身未申请外发、没有自动审批拒绝，不混写。若main前进须重叠放且fresh全量和反例复跑。回滚参考父212c；保留既有来源、费用、水位和历史事实。全部QA/WT/branch保留，仅可回收已停止纯编译缓存。
**历史first阶段（Codex原记录L827–836；原时点、原head与后来的公开/验收head分列）：**
历史段标题：J50 — AG075 原交付当前主线整合实际验收 / PR131 实际 admin 合并（2026-10-11）
历史first事实：- GitHub PR131 实际 MERGED，UTC2026-10-11T04:00:06Z。main=aa831fba2d09d0ef2d0755c3d7a08971b8f6667f，parents=1132b6b4b6e0016540339ba65219daf1ceb35fb8 / 8a92e578527b6334b5fb776abf46c5bc9a88e4b7；tree=2cfe492ce726c7ce1e44ca0cc9736c857b2bc072 与 ROOT 验收树相同。#98–131 共34个实际已合并。
历史first事实：- AG075 六个 Rust 文件 +2860/-0：四个完整产品/测试文件逐字节等于原 b9f7 已独立审查载荷；engine/storage 的两处 lib.rs 只机械叠加原模块注册行，保留当前主线其它字节。不把六文件误写为整文件全等。
历史first事实：- 全新 opt-level1 / debug0 / incremental0 / jobs2 标准8实际全0，120 Rust组1598测试、141未过滤Python、原bareMCP47完整。独立 unchanged normal-API head/parent 都先编译再执行，whole6/whole4全0且无过滤，父线4例为正向旧API边界，不宣称语义红测。全部12项、原始argv/CWD/env/stdout/stderr、真实执行程序及冻结源 full14 均经 ROOT 实物复核。
历史first事实：- 原 r1 卡 private counter 名误带 AG070，启动前已发现；另建 r2 仅纠正为 AG075，原 r1/空targets/源保留。初次 ROOT 封存读取不存在的 checker计数键，真实native1且未写Task；仅以实际 needs_verification 列表长度修正，再封存native0，不改重跑结果。AG086 私有验收准备第一次 mkdir 缺父目录，真实native1发生于任何写入前；仅补 parents=True 后形成既有任务的私有QA SOURCE；没有新产品任务/派单/代理/验证主进程。
历史first事实：- 公共推送、draft创建、ready、固定头admin合并均按本窗口最新“交付通过的admin批准合并，不要再问我授权”在原通道实际执行成功；旧 Oct3拒绝记录保留且不重新标记通过。公共内容仅六文件与冻结PR正文，私有真源/QA/反例不外发。
历史first事实：- PR131合并后只清理已结束的三个本轮targets可再生非执行文件：9087 REG、1616空DIR、allocated估计1946275840B；315个真实程序和Task/RAW/QA/SOURCE/DB继续保持原 full14。历史 J49 四个已合并 fresh8程序的无损gzip归档与原路径退役记录保持不变。
历史first事实：- 当前剩余既有后端：AG086/087/088/089/090/091/092/093（8项）；既有前端a0ea的14文件当前主线整合验收也尚未完成。AG093的54脏文件与SourceSC保留，RuntimeSC/Task仍未通过；不据此宣称收尾完成。下一项为既有AG086三记录索引一致性当前整合。
历史first事实：- 用户最新“收尾完成给我一份交付指南，不要啰嗦”已纳入最终交付要求：实际关闭剩余交付后给简短版本/启动/验收/边界指南。现在不提前写已完成指南。whole v4.2未验收完成；支持索引仍219 needs_verification。持续工具goal历史paused不伪改完成，按用户现有手动收尾授权执行；rsiagent-30已删除，不重建。ROOT旧618 checkout及所有未提交内容保护。
原未归属项的有限补述：AG075 typed依赖诊断的结论和未检查范围段。
原未归属项的有限补述：AG075单Session只读最多202对象点读/10000实际边/页宽256；未穷尽必Partial，Unknown/合法状态与typed fingerprint语义保留。
原未归属项的有限补述：AG075不授全部namespace、live消费或物理blob/文件核查；水位仅快照诊断，不作返回时授权，无CLI/自动修复/持久报告接线。
原未归属项的有限补述：AG075单Session typed只读诊断的局部结论段。
原未归属项的有限补述：AG075限202对象/10000边/256页宽，不穷尽必Partial；状态与typed指纹解释保，无物理blob资格。
原未归属项的有限补述：AG075来源水位仅读取快照，不授live消费；无CLI、修复或持久报告接线，不宣全namespace/所有兄弟闭包。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E08；E08.ag075_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG075-existing-main1132-fixed8a92e57-ROOT-fresh12-runtime-r2-20261011.json`，文件SHA256 `ac0d4dc732ae1b25b0605537dda1cd0c4ccc0e12ea9a800e47cc31c2a671f790`。
- 原卡导航：`out/audit-20260930/cards/AG-075.md`，文件SHA256 `f3a6621e090f0a04810b9a2f63f61e2179bb325e02852237e24611a237befe46`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG075-existing-current1132-fixed8a92e57-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `a66ea7f4ada632a6624d9abc97144e70e2b75b83ee9c41c58b00ed0e9940951d`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `aa67ddf53d47a3f6eea2183f5aaa257550fff98d505e1980b56ae6fcbc34888e`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG075-existing-current1132-fixed8a92e57-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `4b25a83ffbed5e2518ec9710c510ff329f69781bc8bdc9902df6d6614b0fc441`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `aa67ddf53d47a3f6eea2183f5aaa257550fff98d505e1980b56ae6fcbc34888e`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG075-existing-current1132-fixed8a92e57-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/11/complete_stderr_UTF8`，原载荷SHA256 `4b893c5ef1870c52b194e20e4e00410872bfd24300d0d05d1a48b653a7268e72`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `aa67ddf53d47a3f6eea2183f5aaa257550fff98d505e1980b56ae6fcbc34888e`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG075-existing-current1132-fixed8a92e57-ROOT-fresh12-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/11/complete_stdout_UTF8`，原载荷SHA256 `35216dfd86f870c82d7ff5f9624e5f933d01f5f7265d3ee8f6308a326c18abfe`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `aa67ddf53d47a3f6eea2183f5aaa257550fff98d505e1980b56ae6fcbc34888e`。
旧v4.2阶段 B07（H `b9f7bb72bc2c42ee96de2f892c78e40b170d3346`；实际Parent `5ee01d1e2ebb2401fe94530ff286801239940259`；原时点，本地/失败/授权角色与本节最终头分列）：
历史阶段补述：旧b9f7/Parent5ee标准门：9实际MAIN中原workspace foreignCargo3/total4窗口不被接收，原窗口/流/授权保；R8明确裁决后同argv唯一budget-window-r1完成完整再验。正式1371Rust/103groups、Python48及fmt/clippy/build/两smoke实际0，不能折为一次workspace全绿或删除不接受窗口。
历史阶段补述：checker input_binding_available=false/needs219保，所有raw/前后/授权回执在原像导航；原当时仅开始后续ordinary Head/Parent与具名counter，不提前挪后来的counter或main信用。原同段AG070具体push拒绝只属AG070，不拿它给PR131失败数量/外发权限。
精确旧阶段原像导航：`historical-original-ledger` 的 ledger:5006；各完整原LF SHA及文字映在本地R11覆盖；本补述不授新验收。
### PR #132 — AG-086（第四轮历史）
冻结M6角色：公开head 908eb85b33d590889a75885c9d9b20c3dad927b8；本地验收head 908eb85b33d590889a75885c9d9b20c3dad927b8；实际merge 77152d77c96f497ebf2c0123e985f0832fa3cbd0；mergedAt 2026-10-11T04:54:45Z。
合并父 aa831fba2d09d0ef2d0755c3d7a08971b8f6667f, 908eb85b33d590889a75885c9d9b20c3dad927b8；tree 426065da2aefd359b9e2808b67c1be969f46acb7。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E16.6/verified_subscopes/0`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E16.6.ag086_local_scoped_acceptance；description = Partial evidence registry and unchanged historical evidence records
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 908eb85b33d590889a75885c9d9b20c3dad927b8；pr = https://github.com/acosmi/RSIAgent/pull/132；merged_sha = 77152d77c96f497ebf2c0123e985f0832fa3cbd0；pr_state = merged
历史原测试登记：test_entry = crates/evo-core/tests/support_scope.rs；command = cargo test --locked --offline -p evo-core --test support_scope；exit_code = 0
历史原证据登记：input_digest = 7db828d1da99c22ece28a8ea876f60f8af3c0f8ce98ebe95bcfb44f7e4c57356；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-086-908eb85.json；log_path = out/audit-20260930/qa/takeover-20261002/AG086-existing-currentAA831-fixed908eb85-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E16.6; Partial evidence registry and unchanged historical evidence records software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 908eb85b33d590889a75885c9d9b20c3dad927b8; actual merge tree equals accepted tree 426065da2aefd359b9e2808b67c1be969f46acb7. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #132; retain source, costs, revocation history and original QA. Accepted parent aa831fba2d09d0ef2d0755c3d7a08971b8f6667f.
**历史first阶段（Codex原记录L673–681；原时点、原head与后来的公开/验收head分列）：**
历史段标题：2026-10-03T13:13:12.983350+00:00 ROOT AG086固定HEAD完整验收，公共推送自动审查拒绝，本地队列持续
历史first事实：AG086 fixed d45fcc8d6f90e7c86e49490fa35db84fb9e073e8 / parent ddde63c6ab98a2528258d265999bb902a08b0f55 / tree a8ee6105d791e02a489cd0c4f4171301b41ba93a；WT clean，唯一三文件+1497/-4，无checker/product语义变动。ROOT已核完整69旧record raw对象/顺序/版本逐byte同，新增071/114、083/116、085/117三scope共72条。机械折叠只205标题增两#，旧cutoff源end223498、单LF gap与新begin223499明确；frozen561069B869f5d…及337570B4e3b8e…/fold337980Bfdd379…永久保存，不改历史。
历史first事实：四parent先决normal真实0、三missing登记各真实1。最终16条ROOT direct原native全部独立terminal/clock/session/grant/pre-active-post复核：完整8全0，workspace99组1326/0，Python48/0，support_scope14/0；三登记head各0；正常72manifest0，两实际借用digest/跨scope日志篡改各checker1。资源/source207+main207/index皆闭合；最长poll30秒。ROOT taskQA35684B47f9a74d1c7a90be4551e6ab1e3a18be8c946cb307bd6b75fe64a37e5f1811da；独立SC32605Bab85c2aac219409172d5d4525af7fcab9cfaa86c05b8da5aa515d726d543c3be。仅E00/E16.6有限登记，不闭完整E/V/T126/host/model/effect/production/release。
历史first事实：公开三文件完整patch358537B9bd362b4edccc44d3a1372d30497abfb6b339361b269288210b22ebf94d1c8a2及body1824B96678fbe4db5a35ef156cf9904b0f909f926e5d3ee6891528de2a841bc882f38已冻结。fresh fetch真实0、HEAD/origin main ddde、exact远端086branch缺席、PR head列表[]、所有payload/task/SC物理pin及diff复核真实0。
历史first事实：git push origin d45fcc8d…:refs/heads/codex/ag-086-controller-ledger-r4e 被auto_review在CreateProcess前拒绝：现有merge授权未明确涵盖向GitHub acosmi/RSIAgent导出该具体payload。无native进程、无session、无exit；未push、未创建PR、未ready、未merge，PRnumber/mergeSHA仍null。拒绝QA1223B100569a22f8a2f4121a142cffcc29241e3326170fd117df4b491d05bfa7f0392；已在本聊天发精确载荷/目的地async单次批准问题并告知原因，不绕过。原来082/072/056的公共阻塞仍各自保留；不能把admin泛授权或其它PR成功冒签本次外发。
历史first事实：实际main/origin仍ddde、20个r4 PR98–117真正merged，公开main索引仍69；72属于已验收本地candidate。086target暂保留，不冒称merged退休。AG069下一实际main差额5文件manifest86c7a466…仅静态；当前47792d/parent9b/207源及六文件79517B5b88原样，两旧数据库4fdb/5890及原探针只读保持。075继续小型执行源/fixture绑定准备，等待实际安全main卡；无未经授权产品/rebase/runtime。
历史first事实：持续goal ACTIVE，既有rsiagent-30 heartbeat物理ACTIVE；手工实际12:56:51/13:07:55核main/disk，后者Available23480000KiB=24043520000B。历史12:19→12:56超过30分钟如实，不冒称scheduler已实际运行。用户minimum0/低于10不停全局仍优先；当前公开审批并非磁盘阻塞。旧失败诊断/未提交修改/DB/QA/source/index全保护；不收尾不标目标complete。公共original native 15282Bb4ef0a5cb159c20cd652eeaafbac98e3f8c465ddab7c5bea0a6de742b22a8e41已保存。本段两wrong filename/相对WT读失败保留真实错误，不作为接受依据。
**历史first阶段（Codex原记录L837–845；原时点、原head与后来的公开/验收head分列）：**
历史段标题：J51 — AG086 / PR132 实际 admin 合并，网络恢复与独立验收调用纠正（2026-10-11）
历史first事实：- 遵照现有无遗漏无尾巴收尾及验收通过 admin 合并授权，不新增产品任务、编号、代理或派工；定时 rsiagent-30 已删除。收尾完成后给简短交付指南，当前仍有现有存量，whole v4.2不冒记完成。
历史first事实：- AG086 原 d45fcc8d 三文件 +1497/-4逐字节未改，叠放 mainaa831 固定908eb85b33d590889a75885c9d9b20c3dad927b8，树426065da2aefd359b9e2808b67c1be969f46acb7。网络中断前 commit 未实际执行，恢复后实查HEAD仍parent且三文件已暂存。旧严格full14复核失败只因挂载dev16777231→16777233，完整字节及其余metadata逐项相等；原失败和bootstrap保留，另封有限device-only重绑定，随后实际commit成功。未改产品或旧证据。
历史first事实：- 当前标准8真实native0，Rust120组1598测试、Python141含原bareMCP47，无失败/跳过/过滤。原独立whole2真实native101/0通过2失败，原因是ROOT私有Python向checker传入其禁止的绝对manifest参数。逐字核对旧fixture SHA与三个登记tuple均相同；不错误归因代码/hash。仅私有argv改为repo-relative，新SOURCE/新批准记录器/独立fresh target补跑正常API编译及whole2均actual0，同一执行程序；原标准8不重开，原101、RAW、源及记录器不覆盖。
历史first事实：- 两反例实际验证三条原PR114/116/117登记、currentchecker结构0，以及原normal72 fixture0 / wrong-input-digest1 / cross-scope-log1，后两项明确各自预期语义拒绝。旧parent三项missing-registration为历史，不改贴当前parent红测。ROOT读取完整原始流、115个实际workspace Running程序与785REG/139DIR保护。Task2563311/SHA75f92393实物封存。封存器首次print语法错误、其次plan路径笔误均在任何Task写入前native1；有限修正后actual16c4f2/native0。产品/门禁/测试范围未改。
历史first事实：- 专属分支推送、draft132创建/附加/ready及固定head admin合并均在原通道实际成功；旧外发拒绝留存，没有换通道绕过。PR https://github.com/acosmi/RSIAgent/pull/132 ，mergedAt2026-10-11T04:54:45Z。actual main77152d77c96f497ebf2c0123e985f0832fa3cbd0，parents恰aa831/908eb85，合并树严格等于验收树；fetch仅更新origin/main。第四轮#98–132共35项真实合并。
历史first事实：- AG077 PR125已闭三个target程序无损gzip归档和退役真实完成：285程序逐个完整解压字节/SHA相等，archive只读fsync且先封pre-unlink checkpoint后移原程序；三个target已移除，估计净释放1658400768bytes。收据689986/SHA d4f46d8e；原Task/RAW/QA/source/DB保持。旧路径full14此后为退役历史，不能再声称原程序仍在原路径。当前AG086源guard前后不交叠。没有清理其他项目Docker缓存。
历史first事实：- support-scope实际228 needs_verification保留，未验证输入、来源/hosts/许可/真实模型/效果/发布边界不提升。剩余现有后端AG087/088/089/090/091/092/093（7项），另有已授权前端a0ea14文件当前主线整合验收；AG09354脏文件SourceSC保留且RuntimeSC/Task仍空。ROOT旧618干净checkout及全部未提交修改保护，按既有增量顺序继续当前主线复验与条件合并，不新增派工。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E00/E16.6；E16.6.ag086_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG086-existing-mainaa831-fixed908eb85-ROOT-fresh10-runtime-r1-20261011.json`，文件SHA256 `3f2bc99f749acab5739d2824f2aa50fd950d6eeda06dc4e120417d93e5e3fd72`。
- 原卡导航：`out/audit-20260930/cards/AG-086.md`，文件SHA256 `b3e5b6ea85ee4220710f26f2b74244b1a2662056c7322c7d27bb6337adf068b2`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG086-existing-currentAA831-fixed908eb85-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/original_failed_master_complete_results_and_RAW/2/complete_stderr_UTF8`，原载荷SHA256 `401993a02665b180f2c39b1eec91701516a388724b725f44dd59d45fc08a77ea`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `75f92393a9655d8b01cdf9befb0290b90feab395b92cb80432da3746b431c0b5`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG086-existing-currentAA831-fixed908eb85-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/original_failed_master_complete_results_and_RAW/2/complete_stdout_UTF8`，原载荷SHA256 `0ab18dfea9948e2dd59a1b93c657cea6135c0d7b456ac8eb4b65fa588e0424d8`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `75f92393a9655d8b01cdf9befb0290b90feab395b92cb80432da3746b431c0b5`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG086-existing-currentAA831-fixed908eb85-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/original_failed_master_complete_results_and_RAW/9/complete_stderr_UTF8`，原载荷SHA256 `e1892a3c268ea173c2cee856bf1299661cafb02f7abef6e8fde90f4078e62a95`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `75f92393a9655d8b01cdf9befb0290b90feab395b92cb80432da3746b431c0b5`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG086-existing-currentAA831-fixed908eb85-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/original_failed_master_complete_results_and_RAW/9/complete_stdout_UTF8`，原载荷SHA256 `e713dee898f6d581c87ac3d3880b0d397a747142b0c84f314459c511c39517d1`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `75f92393a9655d8b01cdf9befb0290b90feab395b92cb80432da3746b431c0b5`。
### PR #133 — AG-087（第四轮历史）
冻结M6角色：公开head fde3dd4b91e9f6f68f93f7b8fe54ec2e74a08fe8；本地验收head fde3dd4b91e9f6f68f93f7b8fe54ec2e74a08fe8；实际merge 8cbf82cb9a715a8b28070c6a7823b16093865425；mergedAt 2026-10-11T05:13:40Z。
合并父 77152d77c96f497ebf2c0123e985f0832fa3cbd0, fde3dd4b91e9f6f68f93f7b8fe54ec2e74a08fe8；tree 19c1e41131a3624619cc01a4bf7f13c8a151ed9c。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E05/verified_subscopes/4`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E05.ag087_local_scoped_acceptance；description = Formal frozen candidate direct dependency and revocation binding
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = fde3dd4b91e9f6f68f93f7b8fe54ec2e74a08fe8；pr = https://github.com/acosmi/RSIAgent/pull/133；merged_sha = 8cbf82cb9a715a8b28070c6a7823b16093865425；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/formal_candidate_dependency_binding_v42.rs；command = cargo test --locked --offline -p evo-engine --test formal_candidate_dependency_binding_v42；exit_code = 0
历史原证据登记：input_digest = be4f1d4132db5c697fa668429ce432f900496e94726366efe8d2ffa87ea22272；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-087-fde3dd4.json；log_path = out/audit-20260930/qa/takeover-20261002/AG087-existing-current77152-fixedfde3dd4-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E05; Formal frozen candidate direct dependency and revocation binding software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source fde3dd4b91e9f6f68f93f7b8fe54ec2e74a08fe8; actual merge tree equals accepted tree 19c1e41131a3624619cc01a4bf7f13c8a151ed9c. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #133; retain source, costs, revocation history and original QA. Accepted parent 77152d77c96f497ebf2c0123e985f0832fa3cbd0.
**历史first阶段（Codex原记录L846–856；原时点、原head与后来的公开/验收head分列）：**
历史段标题：J52 — AG087 / PR133 实际 admin 合并，已闭target退役与剩余整合预检查（2026-10-11）
历史first事实：- 现有收尾范围保持；无新产品任务/编号/代理/派工，rsiagent-30已取消。按用户已有“验收通过admin合并，不再问授权”执行。已重读startup第2/3.7/6/11节：允许ROOT按双方卡和真源解决合并冲突；在途关联返修允许，最终收尾台账PR、交接及操作记录属于必要交付，不能遗漏。
历史first事实：- AG087 旧2f3558d5四Rust +3651/-12机械叠放main77152，固定fde3dd4b91e9f6f68f93f7b8fe54ec2e74a08fe8，树19c1e41131a3624619cc01a4bf7f13c8a151ed9c。streaming_evaluator及新测试两个整文件原样；release_store、storage/lib两文件与已合并主线同时有改动，Git apply --3way实际clean且逐字等于独立merge-file结果，原增量和主线均保留，不伪称四整文件全等。
历史first事实：- 唯一ROOT全新目标fresh10实际全部native0：121Rust组1622测试（main1598+原新增24），141Python含原bareMCP47，无失败/忽略/过滤；不重复主验证。原57899bytes/SHA b0448c26独立反例未改，正常API编译0并在相同程序whole2无过滤通过。显式签名fixtures输出root由有限辅助在recorder创建case后、实际测试启动前创建，计划/源/记录器未改，辅助实际终态0，精确收据1992/SHA ed0fe580。
历史first事实：- ROOT物理读取全部result/invocation/原stdout/stderr、116真实Running程序、545REG/92DIR，逐个读取所有完整typedSQLite JSON catalog及single-intervention/outcome证明；只收SQLite/WAL元数据，不读/哈希DB正文。两个反例各自3条正式读在S2单墓碑时精确Forbidden、候选sources单缩减时精确formal_candidate_dependency_binding_changed Conflict；每次same-session提交后的全catalog等于干预后全catalog。Task36684968/SHA b710681e已实物接受，AG093原54脏文件full14及status不变；旧Parent红测仅历史。
历史first事实：- 已冻结正文1714/SHA97cb2349，仅原四产品文件外发。专属push、draft133/附加/ready和admin固定fde合并实际成功；PR https://github.com/acosmi/RSIAgent/pull/133 ，mergedAt2026-10-11T05:13:40Z，actualmain8cbf82cb9a715a8b28070c6a7823b16093865425，parents恰77152/fde，merge tree恰验收tree。第四轮#98–133共36项真实已合并。ROOT旧618checkout继续保护，fetch只更新origin/main。
历史first事实：- 已合并结束075三target315程序、070六target400程序、086三target245程序均无损gzip并逐字完整解压核验、先持久化只读checkpoint才退役，净allocated估计850128896/1944399872/754626560bytes。086另仅非执行可再生缓存5553REG、1002381312bytes先清。原Task/RAW/QA/source/DB/Git保持；已退役原程序路径full14为历史，可由各archive恢复原字节，不再声称原路径仍物理存在。全部退役范围与在途087guard不交叠并前后精确。
历史first事实：- 对既有088–092原补丁及093冻结54仅私有SOURCE预检查：088–092叠放预览无冲突；093的streaming_evaluator、storage/budget、storage/lib发现3处文件冲突，尚未修改产品或运行093新验证。预览固定fde且投影尚非actualmain/Task，不用于验收。按双方已有合同解决，将保留既有依赖绑定、budget mode拒绝及093内容授权能力；不以选择单边丢弃另一已通过合同。
历史first事实：- 本机实际12逻辑核/16GiB，无swap；为后续已有验收保留单master、独立全新target，有限将编译jobs2→3。新独立私有记录器r4仅这一literal变化，完整门禁、parser、业务断言、白名单、SOURCE/RAW守卫不变，前端仍不在AG白名单；原r3/在途087不改。资源裁决24171/SHA3fa4b302、r4源码10528/SHAf668831c实际只在本地。
历史first事实：- 余存量后端AG088/089/090/091/092/093（6），另有已授权透明3D状态小人前端14文件及第一真源明确要求的最终收尾台账PR/交接记录。093源54脏文件已保护，当前RuntimeSC/Task空；support实际228 needs_verification保持，不冒认全E/V/真实model/host/效果/发布或wholev4.2完成。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E05；E05.ag087_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG087-existing-main77152-fixedfde3dd4-ROOT-fresh10-runtime-r1-20261011.json`，文件SHA256 `6034b900d0b5ecdd3ab18aa1b260299d053099e26b6dff6c46bbce85f11ad00b`。
- 原卡导航：`out/audit-20260930/cards/AG-087.md`，文件SHA256 `96f99f3734021e07ac3f280bfac24205cfadff97999b5e8a51b068faf065f4e6`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG087-existing-current77152-fixedfde3dd4-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `a78e4ca8b1189a05cab7e9464166b3ebff353e2aae7805f2d11a4dee82e744d2`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `b710681e4412a80be8adeab96680da4507a18c63c912b83b136cd4dd24502393`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG087-existing-current77152-fixedfde3dd4-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `d13640bce72c5cee41f5fd882c6e2d81547bc23ed2cdca9d8a3e8f7131373944`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `b710681e4412a80be8adeab96680da4507a18c63c912b83b136cd4dd24502393`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG087-existing-current77152-fixedfde3dd4-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stderr_UTF8`，原载荷SHA256 `bf85a67414c9b18e3246872353e1c7d15b616c66b4ab29f0c485d82011884d18`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `b710681e4412a80be8adeab96680da4507a18c63c912b83b136cd4dd24502393`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG087-existing-current77152-fixedfde3dd4-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stdout_UTF8`，原载荷SHA256 `d4cab410c289281424e941bb15ba231c8e307010ecaf743c0042578f6410e645`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `b710681e4412a80be8adeab96680da4507a18c63c912b83b136cd4dd24502393`。
旧v4.2阶段 B08（H `44b63eee8440fd312379694241c419f840101ade`；实际Parent `618ad834ff73e878898fb64681a4efc323b84ac0`；原时点，本地/失败/授权角色与本节最终头分列）：
历史阶段补述：旧44b6/Parent618本地Task是原9+R4续3：原dee377→94e564 native1保，R4唯一续3 native0；不是新fresh12全0。Parent原project101/recorder101/receiver1不重跑，仅新receive-only receiver0；Head普通no-run0/countnull、唯一whole2pass0other分列。
历史阶段补述：PASS_LOCAL_FIXED_HEAD_CURRENT_MAIN_618_ORIGINAL9_PLUS_R4_CONTINUATION3仅释放AG087自己的原epoch和续接epoch，原Source/raw及其它Task始终保；当时主线仍618/固定44b6，不能借后来最终头删掉旧失败或宣称全阶段重跑0。
精确旧阶段原像导航：`historical-original-ledger` 的 ledger:7029；ledger:7031；各完整原LF SHA及文字映在本地R11覆盖；本补述不授新验收。
### PR #134 — AG-088（第四轮历史）
冻结M6角色：公开head e538b9664fb9199fead8f2298a27e38da686cd7b；本地验收head e538b9664fb9199fead8f2298a27e38da686cd7b；实际merge bd20099a01c64b87b9a75707a95c785486bb4fda；mergedAt 2026-10-11T05:42:25Z。
合并父 8cbf82cb9a715a8b28070c6a7823b16093865425, e538b9664fb9199fead8f2298a27e38da686cd7b；tree 202b1f627c9cdc0597165b989bfb025bb1a05ea0。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E05/verified_subscopes/5`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E05.ag088_local_scoped_acceptance；description = Complete-batch reports bound to retained actual statistics
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = e538b9664fb9199fead8f2298a27e38da686cd7b；pr = https://github.com/acosmi/RSIAgent/pull/134；merged_sha = bd20099a01c64b87b9a75707a95c785486bb4fda；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/complete_batch_statistics_binding_v42.rs；command = cargo test --locked --offline -p evo-engine --test complete_batch_statistics_binding_v42；exit_code = 0
历史原证据登记：input_digest = 351df7e0a52086493572aef533b888bb0991e3790f64564f70c944f9603aa31c；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-088-e538b96.json；log_path = out/audit-20260930/qa/takeover-20261002/AG088-existing-current8cbf82c-fixede538b96-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E05; Complete-batch reports bound to retained actual statistics software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source e538b9664fb9199fead8f2298a27e38da686cd7b; actual merge tree equals accepted tree 202b1f627c9cdc0597165b989bfb025bb1a05ea0. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #134; retain source, costs, revocation history and original QA. Accepted parent 8cbf82cb9a715a8b28070c6a7823b16093865425.
**历史first阶段（Codex原记录L857–866；原时点、原head与后来的公开/验收head分列）：**
历史段标题：J53 — AG088 / PR134 实际 admin 合并，AG087 两目标无损退役，093冲突私有预解（2026-10-11）
历史first事实：- 原收尾范围不新增任务/编号/代理/派工；用户已授权验收通过admin合并且不再问授权。AG088原485两Rust+1335/-21叠放main8cbf82c，固定e538b9664fb9199fead8f2298a27e38da686cd7b，树202b1f627c9cdc0597165b989bfb025bb1a05ea0。两个整文件原字节一致，旧提交缺skip-ci/coauthor，bootstrap r1在写产品前实际1，旧私有补丁保留；有限r2仅补齐提交元数据后实际0，未改产品内容。
历史first事实：- 唯一ROOT fresh10全部实际native0：122Rust组1624测试、141Python含原bareMCP47，所有失败/忽略/过滤为0。原36562bytes/SHA7fc875ac独立counter完整未改，正常API编译0及同一程序whole2无过滤通过，117真实Running程序物理full14核对；REG551/DIR92与AG093原54未提交源/status全程守卫精确。ROOT原日志、invocation和所有JSONcatalog/全部typedformal envelope/call行实物读取，SQLite只元数据未读或哈希正文。混合72 mean0.75→0.25与v1反转80 reasons单字段干预：三个正式读取均精确Conflict(formal_complete_batch_statistics_changed)且干预后全业务catalog和watermark跨提交读完全保持；146/162已finalized ProgramFixture调用及1314/3483micros如实保留，不称真实外部付费模型。
历史first事实：- 当前Task13536179/SHAff6984a0物理接受；正文1593/SHAb00255ed、两文件公共载荷302796/SHA3f8843d8冻结。实际专属push成功，draft134/附加/ready，实际GitHub main8cb/固定e538/CLEAN/两文件核对后按admin合并。PR https://github.com/acosmi/RSIAgent/pull/134 ，mergedAt2026-10-11T05:42:25Z，actualmain bd20099a01c64b87b9a75707a95c785486bb4fda，父恰8cb/e538，merge tree恰验收tree；fetch实际0。#98–134共37真实合并。
历史first事实：- 已合并AG087双target6981REG非执行可再生缓存安全清除，估计1403453440bytes；254程序lossless gzip完整解压字节SHA核验，每次fsync只读checkpoint先于原程序unlink，双target退役，净allocated估计809590784bytes。全部RAW/QA/Task/source/SQLite保留，退役程序原路径full14是历史、字节可恢复，当前AG088 guard前后精确且不交叠。cleanup443549/SHA1df344bf、archive611148/SHA2a528405。
历史first事实：- AG093冻结54仅私有SOURCE预解，不是actualmain整合或Task：histogram/patience流式文件原样相同clean；budget保留主线legacy/E16 mode拒绝、两E16方法、snapshot活性及仅成功commit，恰两helper调用对应93新helper。旧helper和新helper完整尾体除canonical借用逐字相同，删除未调用旧重复体；93新增caller/reservation/canonical检查、savepoint/refence/source约束保留。storage/lib完整保留两方outgoing_edges_bounded与dependency_edges接口。预解不触碰任何产品工作树或Gitindex，不扩功能。
历史first事实：- 093预解辅助r1路径未配置rustfmt实际1；r2识别旧重复helper实际1；r3/r6私有Python缩进失败；r4 standalone lib mod定位失败；r5保留完整rustfmt带文件名stdout仅作为历史展示输出，不能当产品源码；有限r7只使用固定本机rustfmt路径/skip_children=true，保存原stdout并去除唯一精确文件名头，三个Rust派生SOURCE成功。streaming/lib正文与preformat完全相同；budget仅格式变化，未来须绑定actualmain并完整复验，22909/SHAed0b7051。原失败与部分叶均保留，不改旧54或把预检查报通过。
历史first事实：- 当前AG089已机械叠放实际bd20099，两完整文件与原0b29167一致，提交6040bf338e3a9a929f036c3e57891729e775f6fe、parent bd20099、tree8a091b458aa236242919697613e7442d879c4373。fresh10签名621037/SHAb017f075预检0，唯一主验证session37996于05:46:10UTC启动，尚未Task接收或合并，禁止重复启动或修改冻结源。
历史first事实：- 剩余既有后端089/090/091/092/093（5），原透明3D状态小人前端14文件、最终收尾台账PR与交接/命令记录仍必需。228 needs_verification与不完备输入绑定保持；不称全v4.2、全E/V、真实model/host/改进效果或发布完成。ROOT旧618checkout和所有历史证据保护，timer已取消。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E05；E05.ag088_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG088-existing-main8cbf82c-fixede538b96-ROOT-fresh10-runtime-r1-20261011.json`，文件SHA256 `8c374ea2d854d9dd6b067a015a55682bdaa85b42af5a27d2fbbe81113d508274`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG088-existing-current8cbf82c-fixede538b96-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `55611b7040981d6c88780d3ed2e14a52201f86eb6343951078899c6855af8ace`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `ff6984a0ae79005cf945ba1c179c954a9d9fdbc20e5871755c0b0b566da36eb0`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG088-existing-current8cbf82c-fixede538b96-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `9c14b5900d6ef5a3e464d1b852475337178dfa83a751b044d6ec92c45273e31b`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `ff6984a0ae79005cf945ba1c179c954a9d9fdbc20e5871755c0b0b566da36eb0`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG088-existing-current8cbf82c-fixede538b96-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stderr_UTF8`，原载荷SHA256 `469953e8ba64aee1633741191c6e52071b3eed596c37dda90af2dd95db6c189e`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `ff6984a0ae79005cf945ba1c179c954a9d9fdbc20e5871755c0b0b566da36eb0`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG088-existing-current8cbf82c-fixede538b96-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stdout_UTF8`，原载荷SHA256 `3b2fa613a3858c16ca71dcd0b819640e91947b3b15ecf3f641b4421200ebd73d`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `ff6984a0ae79005cf945ba1c179c954a9d9fdbc20e5871755c0b0b566da36eb0`。
旧v4.2阶段 B09（H `4854607f4a33acbd822eec65bfe510042db25930`；实际Parent `44b63eee8440fd312379694241c419f840101ade`；原时点，本地/失败/授权角色与本节最终头分列）：
历史阶段补述：旧485/Parent44b6唯一Root标准8+ordinary/whole2十步全0：Rust1457/108groups、UT48、ordinarycountnull、自身whole2pass；只本AG088 epoch释放，无automaticretry/resume，不能把作者/后e5381624数字套入此Root执行。
历史阶段补述：Parent44b6是未合主线AG087的本地Task，旧main/origin仍618；main_merge_eligible=false，Parent公共四Rust/2371正文具体授权pending，AG088自身可审载荷/具体授权/届时整合仍待，public/push/PR/merge未执行。原有限卡无源修改/Git/public/paid/provider/SQL权限，不能将localParent当实际main。
精确旧阶段原像导航：`historical-original-ledger` 的 ledger:7114；ledger:7126；ledger:7152；各完整原LF SHA及文字映在本地R11覆盖；本补述不授新验收。
### PR #135 — AG-089（第四轮历史）
冻结M6角色：公开head 6040bf338e3a9a929f036c3e57891729e775f6fe；本地验收head 6040bf338e3a9a929f036c3e57891729e775f6fe；实际merge c9aa71e7516e369672b0e5127e675d72eb199304；mergedAt 2026-10-11T05:57:39Z。
合并父 bd20099a01c64b87b9a75707a95c785486bb4fda, 6040bf338e3a9a929f036c3e57891729e775f6fe；tree 8a091b458aa236242919697613e7442d879c4373。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E05/verified_subscopes/6`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E05.ag089_local_scoped_acceptance；description = Early-stop reports bound to their actual first certificate
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 6040bf338e3a9a929f036c3e57891729e775f6fe；pr = https://github.com/acosmi/RSIAgent/pull/135；merged_sha = c9aa71e7516e369672b0e5127e675d72eb199304；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/early_stop_certificate_binding_v42.rs；command = cargo test --locked --offline -p evo-engine --test early_stop_certificate_binding_v42；exit_code = 0
历史原证据登记：input_digest = e0e3c2ab6317ca52eb600a8cc910d53e0511b32f4eb9afd743153aaf6f806969；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-089-6040bf3.json；log_path = out/audit-20260930/qa/takeover-20261002/AG089-existing-currentbd20099-fixed6040bf3-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E05; Early-stop reports bound to their actual first certificate software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 6040bf338e3a9a929f036c3e57891729e775f6fe; actual merge tree equals accepted tree 8a091b458aa236242919697613e7442d879c4373. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #135; retain source, costs, revocation history and original QA. Accepted parent bd20099a01c64b87b9a75707a95c785486bb4fda.
**历史first阶段（Codex原记录L867–876；原时点、原head与后来的公开/验收head分列）：**
历史段标题：J54 — AG089 / PR135 现有交付当前主线完整验收并实际 admin 合并（2026-10-11）
历史first事实：- 仅完成原收尾范围，无新任务/编号/代理/派工；用户持续授权验收通过按admin合并，无新增待授权。旧交付0b291675c03b66a7407c58508b774c992de2cae6逐字机械叠放实际mainbd20099a01c64b87b9a75707a95c785486bb4fda，固定6040bf338e3a9a929f036c3e57891729e775f6fe，tree8a091b458aa236242919697613e7442d879c4373。两Rust +2010/-0；完整源文件与bootstrap逐字预计值匹配，不改原产品内容；提交元数据按原startup保留skip-ci/coauthor。
历史first事实：- 唯一ROOT全新独立target fresh10实际全部native0，123Rust组1634测试、141Python含原bareMCP47，失败/忽略/过滤0。原独立counter完整64366bytes/SHAb29858c75f4b1262a3c317750175dca903d5b9a731bb0ba90019daa6ef165780未改，只私有target名称alias；正常API编译0和相同程序whole2无过滤0，118真实Running程序实物full14读取。REG556/DIR92及index精确；AG093原54未提交源和status不变。
历史first事实：- 当前独立反例语义实物接收：[{"Some_binding": true, "actual_closed_calls": 30, "actual_fee_micros": 270, "actual_first_stop_k": 13, "actual_first_stop_reason": "futility_quality_gain", "all_three_specific_refusals": "formal_early_stop_certificate_changed", "case": "coordinated-first-stop-reason", "committed_three_read_all_catalogs_unchanged": true, "complete_other_catalog_and_money_exposure_sources_watermark_preserved": true, "fixture_exact_members": ["actual-api-events.production.json", "actual-callback-schedule.json", "actual-candidate-issued-ticket.json", "after-intervention-all-three-outcomes.actual.json", "after-intervention-catalog-00-before.actual.json", "after-intervention-catalog-01-after-status.actual.json", "after-intervention-catalog-02-after-view.actual.json", "after-intervention-catalog-03-after-same-session.actual.json", "after-intervention-same-session-view.actual.json", "after-intervention-status.actual.json", 
"after-intervention-verified-view.actual.json", "after-three-all-money-exposure-source-watermark.actual.json", "final-complete-typed-catalog.actual.json", "final-semantic-assertion.actual.json", "fixture.sqlite3", "frozen-control.actual.json", "frozen-holdout.actual.json", "intervention-all-money-exposure-source-watermark.actual.json", "intervention-catalog-00-before.actual.json", "intervention-catalog-01-after.actual.json", "intervention-exact-raw-row-and-recursive-diff-proof.actual.json", "intervention-original-ticket-and-formal.actual.json", "intervention-proposed-ticket-and-formal.actual.json", "positive-all-money-exposure-source-watermark.actual.json", "positive-all-three-outcomes.actual.json", "positive-catalog-00-before.actual.json", "positive-catalog-01-after-status.actual.json", "positive-catalog-02-after-view.actual.json", "positive-catalog-03-after-same-session.actual.json", "positive-same-session-view.actual.json", 
"positive-status.actual.json", "positive-verified-view.actual.json"], "sole_intervention_allowed_rows": 2}, {"Some_binding": true, "actual_closed_calls": 30, "actual_fee_micros": 270, "actual_first_stop_k": 13, "actual_first_stop_reason": "futility_quality_gain", "all_three_specific_refusals": "formal_early_stop_certificate_changed", "case": "formal-ucb-only", "committed_three_read_all_catalogs_unchanged": true, "complete_other_catalog_and_money_exposure_sources_watermark_preserved": true, "fixture_exact_members": ["actual-api-events.production.json", "actual-callback-schedule.json", "actual-candidate-issued-ticket.json", "after-intervention-all-three-outcomes.actual.json", "after-intervention-catalog-00-before.actual.json", "after-intervention-catalog-01-after-status.actual.json", "after-intervention-catalog-02-after-view.actual.json", "after-intervention-catalog-03-after-same-session.actual.json", 
"after-intervention-same-session-view.actual.json", "after-intervention-status.actual.json", "after-intervention-verified-view.actual.json", "after-three-all-money-exposure-source-watermark.actual.json", "final-complete-typed-catalog.actual.json", "final-semantic-assertion.actual.json", "fixture.sqlite3", "frozen-control.actual.json", "frozen-holdout.actual.json", "intervention-all-money-exposure-source-watermark.actual.json", "intervention-catalog-00-before.actual.json", "intervention-catalog-01-after.actual.json", "intervention-exact-raw-row-and-recursive-diff-proof.actual.json", "intervention-original-ticket-and-formal.actual.json", "intervention-proposed-ticket-and-formal.actual.json", "positive-all-money-exposure-source-watermark.actual.json", "positive-all-three-outcomes.actual.json", "positive-catalog-00-before.actual.json", "positive-catalog-01-after-status.actual.json", "positive-catalog-02-after-view.actual.json", 
"positive-catalog-03-after-same-session.actual.json", "positive-same-session-view.actual.json", "positive-status.actual.json", "positive-verified-view.actual.json"], "sole_intervention_allowed_rows": 1}]。全部原result/invocation/stdout/stderr和counter JSON完整物理读取并封存；持久SQLite只元数据，不读/哈希正文；旧SourceSC仅原增量，历史Parent红不冒作当前父红，原失败保留。
历史first事实：- ROOT当前Task：out/audit-20260930/qa/takeover-20261002/AG089-existing-currentbd20099-fixed6040bf3-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json，11859475bytes/SHA6d8af68e1cdff97e6f1ea47774e3f4a111d50dffe7ee6768daa84951c78077d3。卡：out/audit-20260930/cards/AG089-existing-mainbd20099-fixed6040bf3-ROOT-fresh10-runtime-r1-20261011.json / SHAb017f075e05af0935f0955f743bfb6d2924c2f0a4ed8becc0907c622d2406f05；完整runtime：本地scratch（完整原件由上述Task保管），全8+normal2原结果保留。
历史first事实：- 专属push/draft135/附加/ready实际成功，GitHub实际父bd20099a01c64b87b9a75707a95c785486bb4fda、固定head6040bf338e3a9a929f036c3e57891729e775f6fe、CLEAN及完整两文件核对后按admin固定HEAD合并。PR https://github.com/acosmi/RSIAgent/pull/135 ，mergedAt2026-10-11T05:57:39Z，merged_shac9aa71e7516e369672b0e5127e675d72eb199304，actual parents恰验收parent/head，merge tree恰验收tree8a091b458aa236242919697613e7442d879c4373，任何不等均不继续合并。
历史first事实：- 有限verified子范围：v4.2 E05 stored EarlyStopped first-certificate binding at shared three public readers; legitimate late callbacks preserved.；支持索引仍228needs_verification、输入绑定不完备；不冒认全v4.2、全E/V、真实provider/host、收益、部署或发布验收。ROOT旧checkout、原RAW/QA/Task/source/费用/水位历史全保留。
历史first事实：- 其它既有收尾核对：AG088已闭双target非执行6993REG估计1402585088bytes清除；255程序完整逐字解压SHA核验后先封存checkpoint再退役，净allocated估计819503104bytes，两target已移除，Task程序旧路径full14仅历史、字节可恢复。全部证据及在途089guard精确。GitHub实际最近100PR只读结果核对98–134共37项皆MERGED/无OPEN，此刻135另已MERGED；原前端a0ea14文件现有Node10实际全部pass、cancel/skipped/todo0，仅原前端合同Source/Runtime窄范围，不称当前main集成或完整真实UI状态已验收。
历史first事实：- 剩余既有后端AG090,AG091,AG092,AG093；原前端14文件和第一真源要求的收尾台账PR/交接/命令记录仍需完成，rsiagent-30已取消。旧冻结源与所有未提交修改保护。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 原AG089 r3 workspace1462过/5败、109汇总，五例在127.0.0.1:0绑定setup PermissionDenied，业务未触达、后五门未跑；r3整体FAIL保。r4显式回环权限后workspace1467/0、build与workspace smoke0，CLI readiness实际1且server原因unknown，后checker/UT未跑，r4整体FAIL保。
- 同219源码r5仅CLI显式回环权限→checker→UT，三项真实0、UT48；作者九门是实际r3前三+r4前三+r5后三合取，非一次fresh-nine。接收器对象误比整数native1保；最终新headRoot独立fresh验收另列，不能把运输0或修执行环境借作产品语义修复。
- 历史E归属/记录身份：E05；E05.ag089_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG089-existing-mainbd20099-fixed6040bf3-ROOT-fresh10-runtime-r1-20261011.json`，文件SHA256 `b017f075e05af0935f0955f743bfb6d2924c2f0a4ed8becc0907c622d2406f05`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG089-existing-currentbd20099-fixed6040bf3-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `76ba390b186abdf03288735b15cd4b31d1e46355de26e58b3b60b4f1cd0587b2`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `6d8af68e1cdff97e6f1ea47774e3f4a111d50dffe7ee6768daa84951c78077d3`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG089-existing-currentbd20099-fixed6040bf3-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `5765c36154aaa80bbb4017c20eb866a79c654d070bdebb08c82a02590f3cbea9`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `6d8af68e1cdff97e6f1ea47774e3f4a111d50dffe7ee6768daa84951c78077d3`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG089-existing-currentbd20099-fixed6040bf3-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stderr_UTF8`，原载荷SHA256 `d61947148987456c9d31f7735d64e6115320f1e213806e97c907bbe03d89da9a`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `6d8af68e1cdff97e6f1ea47774e3f4a111d50dffe7ee6768daa84951c78077d3`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG089-existing-currentbd20099-fixed6040bf3-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stdout_UTF8`，原载荷SHA256 `6d58d2ca0a8c1fdb633270af73a118fdac2bf1e79a60c7be12ae8b8fd9691a0e`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `6d8af68e1cdff97e6f1ea47774e3f4a111d50dffe7ee6768daa84951c78077d3`。
旧v4.2阶段 B10（H `0b291675c03b66a7407c58508b774c992de2cae6`；实际Parent `4854607f4a33acbd822eec65bfe510042db25930`；原时点，本地/失败/授权角色与本节最终头分列）：
历史阶段补述：旧0b29/Parent485、main/origin618：Root独立fresh标准8+ordinary0+whole2完整十阶段全0，Rust1467/109groups、Python48/OK、whole2两真实语义拒绝均2pass、no-run countnull。它与本节已保作者r4同1467但整体FAIL是不同角色/阶段，不能用同数覆盖任一失败或Root完整通过。
历史阶段补述：旧Scope仍ProgramFixture/TicketExecutionOnly/NamespaceWatermarkOnly、promotion=false，checker input_binding_available=false；不补造生产/真实provider/收益。原Root完成阶段只作旧固定头本地证据，不授后来最终头或当前整体E/V/二验信用。
精确旧阶段原像导航：`historical-original-ledger` 的 ledger:7245；各完整原LF SHA及文字映在本地R11覆盖；本补述不授新验收。
### PR #136 — AG-090（第四轮历史）
冻结M6角色：公开head a9944434b45f4c59c46329a69e5d9c8be4a60ac8；本地验收head a9944434b45f4c59c46329a69e5d9c8be4a60ac8；实际merge 1f1b593d5475043e707e17195222d82f2c5281eb；mergedAt 2026-10-11T06:21:41Z。
合并父 c9aa71e7516e369672b0e5127e675d72eb199304, a9944434b45f4c59c46329a69e5d9c8be4a60ac8；tree 105ca5015416343c040597f44e7e83031323dad9。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E05/verified_subscopes/7`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E05.ag090_local_scoped_acceptance；description = Invalid formal reasons bound to actual paid Some-ticket facts
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = a9944434b45f4c59c46329a69e5d9c8be4a60ac8；pr = https://github.com/acosmi/RSIAgent/pull/136；merged_sha = 1f1b593d5475043e707e17195222d82f2c5281eb；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/invalid_formal_reason_binding_v42.rs；command = cargo test --locked --offline -p evo-engine --test invalid_formal_reason_binding_v42；exit_code = 0
历史原证据登记：input_digest = 1759655da2268ffe9071f656f15c6cc3137fd41404cc88da0aba796432293c2f；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-090-a994443.json；log_path = out/audit-20260930/qa/takeover-20261002/AG090-existing-currentc9aa71e-fixeda994443-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E05; Invalid formal reasons bound to actual paid Some-ticket facts software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source a9944434b45f4c59c46329a69e5d9c8be4a60ac8; actual merge tree equals accepted tree 105ca5015416343c040597f44e7e83031323dad9. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #136; retain source, costs, revocation history and original QA. Accepted parent c9aa71e7516e369672b0e5127e675d72eb199304.
**历史first阶段（Codex原记录L877–886；原时点、原head与后来的公开/验收head分列）：**
历史段标题：J55 — AG090 / PR136 现有交付当前主线完整验收并实际 admin 合并（2026-10-11）
历史first事实：- 仅完成原收尾范围，无新任务/编号/代理/派工；用户持续授权验收通过按admin合并，无新增待授权。旧交付3d332a4b61df9f7560c91d7843566b7bf37cc117逐字机械叠放实际mainc9aa71e7516e369672b0e5127e675d72eb199304，固定a9944434b45f4c59c46329a69e5d9c8be4a60ac8，tree105ca5015416343c040597f44e7e83031323dad9。两Rust +1293/-1；完整源文件与bootstrap逐字预计值匹配，不改原产品内容；提交元数据按原startup保留skip-ci/coauthor。
历史first事实：- 唯一ROOT全新独立target fresh10实际全部native0，124Rust组1641测试、141Python含原bareMCP47，失败/忽略/过滤0。原独立counter完整58773bytes/SHAcb406b3793eeef6aacf1236fcdabd522535b382ebbb4a48fe0efd4689a27739b未改，只私有target名称alias；正常API编译0和相同程序whole2无过滤0，119真实Running程序实物full14读取。REG560/DIR92及index精确；AG093原54未提交源和status不变。
历史first事实：- 当前独立反例语义实物接收：[{"Some_binding": true, "actual_closed_calls": 2, "actual_fee_micros": 18, "actual_ticket_invalid_reasons": ["environment_mismatch"], "all_three_specific_refusals": "formal_invalid_outcome_changed", "case": "empty-formal-reasons", "committed_three_read_all_catalogs_unchanged": true, "complete_other_catalog_and_money_exposure_sources_watermark_preserved": true, "fixture_exact_members": ["actual-candidate-issued-ticket.json", "actual-complete-public-API-event-log.json", "actual-invalidate-writer-result.json", "actual-running-ticket-before-invalidate.json", "after-intervention-all-three-outcomes.actual.json", "after-intervention-catalog-00-before.actual.json", "after-intervention-catalog-01-after-status.actual.json", "after-intervention-catalog-02-after-view.actual.json", "after-intervention-catalog-03-after-same-session.actual.json", "after-intervention-same-session-view.actual.json", 
"after-intervention-status.actual.json", "after-intervention-verified-view.actual.json", "after-three-all-money-exposure-source-watermark.actual.json", "final-complete-typed-catalog.actual.json", "final-semantic-assertion.actual.json", "fixture.sqlite3", "frozen-control.actual.json", "frozen-holdout.actual.json", "intervention-all-money-exposure-source-watermark.actual.json", "intervention-catalog-00-before.actual.json", "intervention-catalog-01-after.actual.json", "intervention-exact-raw-row-and-recursive-diff-proof.actual.json", "intervention-original-ticket-and-formal.actual.json", "intervention-proposed-formal.actual.json", "positive-all-money-exposure-source-watermark.actual.json", "positive-all-three-outcomes.actual.json", "positive-catalog-00-before.actual.json", "positive-catalog-01-after-status.actual.json", "positive-catalog-02-after-view.actual.json", "positive-catalog-03-after-same-session.actual.json", 
"positive-same-session-view.actual.json", "positive-status.actual.json", "positive-verified-view.actual.json"], "sole_intervention_allowed_rows": 1}, {"Some_binding": true, "actual_closed_calls": 2, "actual_fee_micros": 18, "actual_ticket_invalid_reasons": ["environment_mismatch"], "all_three_specific_refusals": "formal_invalid_outcome_changed", "case": "substitute-valid-formal-reason", "committed_three_read_all_catalogs_unchanged": true, "complete_other_catalog_and_money_exposure_sources_watermark_preserved": true, "fixture_exact_members": ["actual-candidate-issued-ticket.json", "actual-complete-public-API-event-log.json", "actual-invalidate-writer-result.json", "actual-running-ticket-before-invalidate.json", "after-intervention-all-three-outcomes.actual.json", "after-intervention-catalog-00-before.actual.json", "after-intervention-catalog-01-after-status.actual.json", "after-intervention-catalog-02-after-view.actual.json", 
"after-intervention-catalog-03-after-same-session.actual.json", "after-intervention-same-session-view.actual.json", "after-intervention-status.actual.json", "after-intervention-verified-view.actual.json", "after-three-all-money-exposure-source-watermark.actual.json", "final-complete-typed-catalog.actual.json", "final-semantic-assertion.actual.json", "fixture.sqlite3", "frozen-control.actual.json", "frozen-holdout.actual.json", "intervention-all-money-exposure-source-watermark.actual.json", "intervention-catalog-00-before.actual.json", "intervention-catalog-01-after.actual.json", "intervention-exact-raw-row-and-recursive-diff-proof.actual.json", "intervention-original-ticket-and-formal.actual.json", "intervention-proposed-formal.actual.json", "positive-all-money-exposure-source-watermark.actual.json", "positive-all-three-outcomes.actual.json", "positive-catalog-00-before.actual.json", "positive-catalog-01-after-status.actual.json", 
"positive-catalog-02-after-view.actual.json", "positive-catalog-03-after-same-session.actual.json", "positive-same-session-view.actual.json", "positive-status.actual.json", "positive-verified-view.actual.json"], "sole_intervention_allowed_rows": 1}]。全部原result/invocation/stdout/stderr和counter JSON完整物理读取并封存；持久SQLite只元数据，不读/哈希正文；旧SourceSC仅原增量，历史Parent红不冒作当前父红，原失败保留。
历史first事实：- ROOT当前Task：out/audit-20260930/qa/takeover-20261002/AG090-existing-currentc9aa71e-fixeda994443-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json，6380453bytes/SHA45b0232c119739d83a8ea4ed3631ccd87069344c57f13ca75062fd9c659f8389。卡：out/audit-20260930/cards/AG090-existing-mainc9aa71e-fixeda994443-ROOT-fresh10-runtime-r1-20261011.json / SHAac1451f1fa08125609b8bb9c9bebaefa72328d1381e9b8989357d257d823a5b6；完整runtime：本地scratch（完整原件由上述Task保管），全8+normal2原结果保留。
历史first事实：- 专属push/draft136/附加/ready实际成功，GitHub实际父c9aa71e7516e369672b0e5127e675d72eb199304、固定heada9944434b45f4c59c46329a69e5d9c8be4a60ac8、CLEAN及完整两文件核对后按admin固定HEAD合并。PR https://github.com/acosmi/RSIAgent/pull/136 ，mergedAt2026-10-11T06:21:41Z，merged_sha1f1b593d5475043e707e17195222d82f2c5281eb，actual parents恰验收parent/head，merge tree恰验收tree105ca5015416343c040597f44e7e83031323dad9，任何不等均不继续合并。
历史first事实：- 有限verified子范围：Local invalid formal reasons bound to actual paid Some-ticket facts at three shared readers; no broader E/V/provider/effect acceptance.；支持索引仍228needs_verification、输入绑定不完备；不冒认全v4.2、全E/V、真实provider/host、收益、部署或发布验收。ROOT旧checkout、原RAW/QA/Task/source/费用/水位历史全保留。
历史first事实：- 其它既有收尾核对：AG089两个结束targets实际删除7005REG非执行缓存并逐字lossless gzip归档256程序后退休；释放非执行约1406320640B及程序净830574592B。旧Task程序路径现为历史可还原证据，不冒作当前仍存在。前端原a0ea实际Node10/原生浏览器12全部pass，透明祖先7层/caption0及胶囊24px/无overflow已实际核对；这些仅原前端呈现合同/显式模拟状态，不是当前main Task、不冒作全状态真实业务。自有浏览器tab关闭、preview会话4712 Ctrl-C实际exit0，未留后台进程。AG091计划制备第1次仅bootstrap目录参数误传导致NotADirectoryError before mutation，r2只更正参数通过；失败原样保留，不更改产品或放宽guard。
历史first事实：- 剩余既有后端AG091,AG092,AG093；原前端14文件和第一真源要求的收尾台账PR/交接/命令记录仍需完成，rsiagent-30已取消。旧冻结源与所有未提交修改保护。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E05；E05.ag090_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG090-existing-mainc9aa71e-fixeda994443-ROOT-fresh10-runtime-r1-20261011.json`，文件SHA256 `ac1451f1fa08125609b8bb9c9bebaefa72328d1381e9b8989357d257d823a5b6`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG090-existing-currentc9aa71e-fixeda994443-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `bf3ac9cec0f65fefc4d864baf1f5f6bef808356da3520a6159eeb46ea942ec8e`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `45b0232c119739d83a8ea4ed3631ccd87069344c57f13ca75062fd9c659f8389`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG090-existing-currentc9aa71e-fixeda994443-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `b0853912b65613bff91c87c64b6664b5a1f3c63fa9d5542ed22f3c3b5531076a`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `45b0232c119739d83a8ea4ed3631ccd87069344c57f13ca75062fd9c659f8389`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG090-existing-currentc9aa71e-fixeda994443-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stderr_UTF8`，原载荷SHA256 `33cec089804ea9112b4ad39cc09d92555808f14b07c7bc1e694f080b4d58520b`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `45b0232c119739d83a8ea4ed3631ccd87069344c57f13ca75062fd9c659f8389`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG090-existing-currentc9aa71e-fixeda994443-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stdout_UTF8`，原载荷SHA256 `0fceebb66608e1c3027a8fbfcc9777067a3c7c50ff146eda0a6aaf9410b868d0`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `45b0232c119739d83a8ea4ed3631ccd87069344c57f13ca75062fd9c659f8389`。
旧v4.2阶段 B11（H `3d332a4b61df9f7560c91d7843566b7bf37cc117`；实际Parent `0b291675c03b66a7407c58508b774c992de2cae6`；原时点，本地/失败/授权角色与本节最终头分列）：
历史阶段补述：旧3d33/Parent0b29本地Task：Root十阶段受理，Rust1474/110groups、Python141含bare47、ordinary0/countnull、原whole2/2pass0other，Parent两最后语义红及SourceSC原F1窄修/必要RuntimeSC链分别保。
历史阶段补述：只释放AG090自己epoch；原Source/raw/DB和失败史不改，不是origin529主线整合信用，旧阶段未PR/merge、不释放其它任务。后来最终头验收与此本地1474分列，不拿现态补写旧公共通过。
精确旧阶段原像导航：`historical-original-ledger` 的 ledger:7330；各完整原LF SHA及文字映在本地R11覆盖；本补述不授新验收。
### PR #137 — AG-091（第四轮历史）
冻结M6角色：公开head 730f242cd4fe076c4d751e3f823cc8a9385fbb93；本地验收head 730f242cd4fe076c4d751e3f823cc8a9385fbb93；实际merge 27df8c2cfccbefa6544b55b697c10abbb6c0a61c；mergedAt 2026-10-11T06:35:29Z。
合并父 1f1b593d5475043e707e17195222d82f2c5281eb, 730f242cd4fe076c4d751e3f823cc8a9385fbb93；tree 199c75ef378da51ea954af656545f31a260ddc6f。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E05/verified_subscopes/8`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E05.ag091_local_scoped_acceptance；description = Confirmed early stop preserved after a late grader error
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 730f242cd4fe076c4d751e3f823cc8a9385fbb93；pr = https://github.com/acosmi/RSIAgent/pull/137；merged_sha = 27df8c2cfccbefa6544b55b697c10abbb6c0a61c；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/late_grader_stop_preservation_v42.rs；command = cargo test --locked --offline -p evo-engine --test late_grader_stop_preservation_v42；exit_code = 0
历史原证据登记：input_digest = 549180e578ecab85419fb93cdf0d4087f4700278cb5605ce2f3610c3e570384c；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-091-730f242.json；log_path = out/audit-20260930/qa/takeover-20261002/AG091-existing-current1f1b593-fixed730f242-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E05; Confirmed early stop preserved after a late grader error software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 730f242cd4fe076c4d751e3f823cc8a9385fbb93; actual merge tree equals accepted tree 199c75ef378da51ea954af656545f31a260ddc6f. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #137; retain source, costs, revocation history and original QA. Accepted parent 1f1b593d5475043e707e17195222d82f2c5281eb.
**历史first阶段（Codex原记录L887–892；原时点、原head与后来的公开/验收head分列）：**
历史段标题：J56 — AG091 / PR137 实际验收、admin合并（2026-10-11）
历史first事实：- 固定head 730f242cd4fe076c4d751e3f823cc8a9385fbb93，parent 1f1b593d5475043e707e17195222d82f2c5281eb，验收及实际合并tree 199c75ef378da51ea954af656545f31a260ddc6f，merge 27df8c2cfccbefa6544b55b697c10abbb6c0a61c，UTC 2026-10-11T06:35:29Z。原两Rust增量+1547/-30保持原文件字节。
历史first事实：- 当前唯一fresh10均实际native0；Rust 1649 / Python141 / 正常API whole2；源及AG093原54修改保护不变。完整结果和语义证明只引用已冻结Task，不重复正文：out/audit-20260930/qa/takeover-20261002/AG091-existing-current1f1b593-fixed730f242-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；SHAaf3233edb0fa0033b0528f90d584c93c9f6f79392f4d6ff6a873d4b95ee29476。
历史first事实：- AG090已合并两个target缓存清理及程序无损gzip封存完成；原前端a0ea SOURCE-only Node10/原生浏览器12证明已接收，当前主线验收仍待。用户指出收尾超过10小时并质疑过度化，停止重复已有合格结果和额外证据扩写，只完成既有交付所需门禁、合并和最终交接。
历史first事实：- 剩余后端AG092,AG093，已有前端14文件、最终台账及交接待闭环。没有新增任务/派工/代理；定时已取消；整个v4.2未完成。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E05；E05.ag091_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG091-existing-main1f1b593-fixed730f242-ROOT-fresh10-runtime-r1-20261011.json`，文件SHA256 `375bf718fe76b74e38388c9c692df6903020f9cef23c7493108c6be9d0fc8ee8`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG091-existing-current1f1b593-fixed730f242-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `61307d029f67d306e88b4f3f0be063a30d2a5348811cd7a7f3f48e694ca9259b`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `af3233edb0fa0033b0528f90d584c93c9f6f79392f4d6ff6a873d4b95ee29476`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG091-existing-current1f1b593-fixed730f242-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `a0cd3549462404e3ad6ac0ba29492bcd831ec48209028283966f0512d1f7b343`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `af3233edb0fa0033b0528f90d584c93c9f6f79392f4d6ff6a873d4b95ee29476`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG091-existing-current1f1b593-fixed730f242-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stderr_UTF8`，原载荷SHA256 `5b66f60fcebc1bc88f0aee78d459167a6f51824bd984940ff6d39d1301da09de`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `af3233edb0fa0033b0528f90d584c93c9f6f79392f4d6ff6a873d4b95ee29476`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG091-existing-current1f1b593-fixed730f242-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stdout_UTF8`，原载荷SHA256 `a0ed220c3b11acd3d757ce3441cd10c30f1ebb04b684d9e9bc000d45b6d49542`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `af3233edb0fa0033b0528f90d584c93c9f6f79392f4d6ff6a873d4b95ee29476`。
旧d52c/Parent3d本地阶段（2026-10-08；区别于730f最终主线验收；以下是原时点完整原句，不授新验收）：
旧阶段原句 ledger:7455：AG091 fixedH=d52c4365132551e257d59d80685ff786269b48c8，P=3d332a4b61df9f7560c91d7843566b7bf37cc117，tree=c8bd9ef48541170394cb0d6e7e90bed183f660e2。ROOT唯一fresh10按签卡实际执行全部十阶段：fmt、clippy、workspace、build、新CLI smoke、workspace smoke、support scope、完整未过滤Python、counter普通构建、同二进制whole2，所有实际project native0、guard_failure=null。workspace106binary+5doc/111groups共1482passed，所有nonpassing0；Python141discovered/run/passed，原bareMCP47绑定本次新CLI；独立counter2另2passed/0failed/0filtered。Whole的实际Running路径、普通阶段compiled bin和前后full14已由ROOT重建核对一致，未重跑任何阶段。
旧阶段原句 ledger:7477：持续目标ACTIVE/unlimited/UNFINISHED。ROOT d71177/native0正式受理AG091必要RuntimeSC R1/R2，receipt40058/376a88ea…；independent exact3报告16471/c74bb2d6…、inputs2948413/b59cf549…、genuine461362/bafb8cb1…，final constructor session60972→902ce8/native0。22个自然工具完整原参/原返含5次解析native1保留，实际运行20条外层原件、10实际阶段及全部原流绑定核实。1139REG/193DIR/38DB lstat和ownindex严格精确；独立重算Rust1482/106binary+5doc、Python141含bare47、counter2、22catalog/5004rows/38978cells、两案九项均true，Parentchild101/capture98失败史保留。inputs的signed_genuinely_empty_target_preparation2原null副本不改；正确card字段genuinely_empty_target_preparation两条actualempty已在reviewer natural5/native0原返存在，ROOT直接核签卡正确字段并引用，不改封包。
旧v4.2阶段 B01（H `d52c4365132551e257d59d80685ff786269b48c8`；实际Parent `3d332a4b61df9f7560c91d7843566b7bf37cc117`；原时点，本地/失败/授权角色与本节最终头分列）：
历史阶段补述：旧ROOT唯一fresh10实际全0：106binary+5doc/111groups、1482passed、Python141含bare47、独立whole2另2pass；这里只证明原d52c/Parent3d本地阶段，不能拿PR125同数或后来730f/1649替代。
历史阶段补述：Parent project child101与capture98后处理失败分别保；不能把capture98记作outer101或recorder运行成功。必要RuntimeSC原22自然工具含5次解析native1原件保，后d71177受理与原失败分列；旧null副本不改，实际empty字段另有原返。
精确旧阶段原像导航：`historical-original-ledger` 的 ledger:7386；ledger:7455；ledger:7477；各完整原LF SHA及文字映在本地R11覆盖；本补述不授新验收。
### PR #138 — AG-092（第四轮历史）
冻结M6角色：公开head 39d2d865c203ca45dd278efa591c934abffb0165；本地验收head 39d2d865c203ca45dd278efa591c934abffb0165；实际merge 77ef86294b7760fc7495d12d4029ca8a3567bc5e；mergedAt 2026-10-11T07:04:57Z。
合并父 27df8c2cfccbefa6544b55b697c10abbb6c0a61c, 39d2d865c203ca45dd278efa591c934abffb0165；tree 8780055d1bccd9006af8d0133fff613249bfc963。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E02/verified_subscopes/3`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E02.ag092_local_scoped_acceptance；description = Fixed parent/baseline input known revoked direct dependencies
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 39d2d865c203ca45dd278efa591c934abffb0165；pr = https://github.com/acosmi/RSIAgent/pull/138；merged_sha = 77ef86294b7760fc7495d12d4029ca8a3567bc5e；pr_state = merged
历史原测试登记：test_entry = crates/evo-core/tests/baseline_dependency_revocation_v42.rs；command = cargo test --locked --offline -p evo-core --test baseline_dependency_revocation_v42；exit_code = 0
历史原证据登记：input_digest = 12418343e9751e0d6b84a0e2f74c80c99875949aa40362dc6b37497d3706f426；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-092-39d2d86.json；log_path = out/audit-20260930/qa/takeover-20261002/AG092-existing-current27df8c2-fixed39d2d86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E02; Fixed parent/baseline input known revoked direct dependencies software/component boundary only.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 39d2d865c203ca45dd278efa591c934abffb0165; actual merge tree equals accepted tree 8780055d1bccd9006af8d0133fff613249bfc963. The named Rust target is included in the recorded whole workspace execution; original independent boundary cases and failures retain their recorded scope. Complete result/RAW bindings are in the immutable ROOT Task receipt. This declaration does not execute the checker grammar command or treat retired compiled paths as present.
历史原风险与不可声明：risk = Partial software evidence only; no transitive current full route, ordinary-P OS principal, real provider/production host, model benefit, promotion, deployment or whole E/V/v4.2 acceptance. Historical parent outcomes stay historical; program paths in earlier receipts may have been retired after owned-target cleanup.
历史原回滚约束：rollback = Reviewed revert of PR #138; retain source, costs, revocation history and original QA. Accepted parent 27df8c2cfccbefa6544b55b697c10abbb6c0a61c.
**历史first阶段（Codex原记录L893–899；原时点、原head与后来的公开/验收head分列）：**
历史段标题：J57 — AG092 / PR138 实際验收、admin合并（2026-10-11）
历史first事实：- 固定head 39d2d865c203ca45dd278efa591c934abffb0165，parent 27df8c2cfccbefa6544b55b697c10abbb6c0a61c，tree 8780055d1bccd9006af8d0133fff613249bfc963，实际merge 77ef86294b7760fc7495d12d4029ca8a3567bc5e，UTC 2026-10-11T07:04:57Z。原两文件+577/-0保持原交付字节。
历史first事实：- fresh10均实际native0；Rust 1662 / Python141 / 正常API whole2。完整结果及语义引用冻结Task out/audit-20260930/qa/takeover-20261002/AG092-existing-current27df8c2-fixed39d2d86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json，SHAe5e631132fcbfe7964ee2aae709d37d1e443a35672f8eac0393d78e87ce403b4，不重复展开证据。
历史first事实：- AG091已合并已结束的两个可再生target实际删除7287文件/约2667790336分配bytes；源/QA/RAW/持久SQLite保留，旧Task程序路径标记历史退役，不新增执行程序压缩归档。按startup3.7.5/6减少过度整理。092预检第1次仅CLI参数位置错误FileNotFound before任何runtime，r2完整路径/bytes/SHA参数修正后实际guardPASS，再唯一master10全0。AG093仅预备54文件整合SOURCE：49原字节、2Git自动合并、3已审双方合同合并；原54修改未动，尚无当前Task。
历史first事实：- 剩余后端AG093，已有前端14文件、最终台账及交接待闭环。未新增任务/代理/派工；定时取消；整个v4.2未完成。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E02；E02.ag092_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG092-existing-main27df8c2-fixed39d2d86-ROOT-fresh10-runtime-r1-20261011.json`，文件SHA256 `cab671a9e9d85e9b92137767db68e6dfc143b1dc6107233aad72ed59db41ad8e`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG092-existing-current27df8c2-fixed39d2d86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `0f5860daf9cfc9fbc05a232bdd4b07c9da11012138c96cde429073fa1148d2ae`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `e5e631132fcbfe7964ee2aae709d37d1e443a35672f8eac0393d78e87ce403b4`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG092-existing-current27df8c2-fixed39d2d86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `60c9b4a14d4f8fd05bee29fe6f1ee610380824f72da5e03c2dabc3aaaf1ac8e8`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `e5e631132fcbfe7964ee2aae709d37d1e443a35672f8eac0393d78e87ce403b4`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG092-existing-current27df8c2-fixed39d2d86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stderr_UTF8`，原载荷SHA256 `d0221d5974ebda6e4099f7df13c5608e26700791f1771633f5e3c41eabeea661`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `e5e631132fcbfe7964ee2aae709d37d1e443a35672f8eac0393d78e87ce403b4`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG092-existing-current27df8c2-fixed39d2d86-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stdout_UTF8`，原载荷SHA256 `9e9cb45b0d28259c9f29d9c97b7a8da78c7795c5a6a068f14e0fe333e636e30e`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `e5e631132fcbfe7964ee2aae709d37d1e443a35672f8eac0393d78e87ce403b4`。
旧0216/Parent529本地与公共拒绝阶段（2026-10-08；区别于39d最终主线验收；以下是原时点完整原句，不授新验收）：
旧Parent529先行反例原句 ledger:7483：AG092 ROOT835609/native0签唯一Parent有限两阶段实卡465283/a36e476c…，真实新空target、475REG/95DIR保护。第一阶段capture14468/project14471，ordinary --no-run真实0，exec7d053b/session95914→dba2b9/native0；ROOT02edfc/native0核完整argv/env/CWD/PIDs/raw和compiled bin1819136/7386d502…正式收。第二阶段capture15098/project15099，exec91cc61/session97997→21d3cd/实际child101与capture101，guard_failure=null。Whole完整stderr唯一Running路径与ordinary同bin full14一致；0passed/2failed/0ignored/0measured/0filtered，唯一panic两行86/163均为最后预期语义断言。
旧Parent529先行反例原句 ledger:7485：ROOT846842/native0完整重建两状态marker：旧compile_skill在Reset baseline正文但继承P空dependencies时，仍对已知dep-b返回Ok；旧compile_bundle在Inherit P正文但Set([])时仍对dep-p返回Ok。三healthy对照完整序列和全部输入不变assert已在两最后panic前实际执行完成；actual完整成功payload与healthy全文解析一致。ROOT按原serde顺序重新核13origin value摘要和一个unsigned完整bundle摘要，未用setup/type/compile/guard失败冒作Parent红。普通阶段的rust-objcopy缺libLLVM非致命警告完整保留，不称warningfree；本例没有DB/网络或付费动作。
旧阶段原句 ledger:7537：单次Git commit PID61159/native0，H=0216e0197c839e2bb10112c91de656797656cbc5，P=5295769892af37988d6bc739bdfb49a834a3f7aa，tree=32b84341ccae1b391661d2306c8beda320bd3c6f，branch codex/ag-092-input-baseline-dependency-revocation，clean；两Rust+577/-0（contract+6，新test+571），规定[skip ci]/Co-Authored-By完整。ownindex仅metadata，inode60147393→60308094(add)→60308138(commit)，branchref inode60147381→60308163且内容为新H+LF；其它ownGit/source218/44保持。无二次fmt/修源/amend/rebase/reset/clean/public。
旧阶段原句 ledger:7600：ROOT8e99ff/native0物理受理，685REG/174闭DIR/3DBmetadata/index与四Parent比较原件、exact3及leaf当前完整full14均相同；十次唯一launch/20事件、真实argv/CWD/env/PID、108Rust汇总1446passed、Python141/bare47与7坏帧预期exit1、ordinary/whole同一二进制、13origin和1bundle独立重算证据成立。Head审查受理收据15792/3ec3bfd6。ROOT签16976/3e087e3a本地Task卡AG092-ROOT-fixed0216-local-Task-accepted-fresh-standard8-independent-counter2-only-own-epoch-release-r1-20261008.json：固定H0216e0197c839e2bb10112c91de656797656cbc5/P5295769892af37988d6bc739bdfb49a834a3f7aa/tree32b84341ccae1b391661d2306c8beda320bd3c6f，两个Rust文件+577/-0，源码和ownGit干净、作者9个实际child全0，原首次作者outer98与Parent whole101/capture101分别保留。SourceSC8、作者固定提交、HeadQA R1、ROOT fresh8+独立counter2、必要Head RuntimeSC均受理，只释放AG092自己的验证epoch；所有源码、历史和其它任务epoch保持保护。
旧阶段原句 ledger:7621：AG092 ROOT8004f0/native0在Task后冻结可审两Rust外发载荷：public-payload.reviewable.json260743/05674a86、PR-body.reviewable.txt1582/74a2e4e4、patch28679/91fe577d；只含已受理H0216的两代码文件+577/-0。实际880426/native0只读远端main仍529、AG092专属branch PR[]且ls-remote空，AG084 PR[]、origin目的正确。其后一次具体git push在CreateProcess前被自动审批拒绝，没有子进程、native退出码或推送执行；不得绕过/换路重试。原拒绝明确认为现有一般admin合并批准及其它任务授权不覆盖这批两本地Rust文件对acosmi/RSIAgent分支codex/ag092-input-baseline-dependency-revocation-0216的具体代码外发。ROOT c82442/native0封存真实错误原文/参数与载荷绑定，2902/2dc3b76b；已按自动审批要求向用户提出该两文件、1582正文及条件admin合并的具体确认，工具accepted、回复尚待。这个阻塞只影响本项公共外发，不影响AG093或总体持续目标；PR/merge仍null。
旧v4.2阶段 B02（H `0216e0197c839e2bb10112c91de656797656cbc5`；实际Parent `5295769892af37988d6bc739bdfb49a834a3f7aa`；原时点，本地/失败/授权角色与本节最终头分列）：
历史阶段补述：旧Parent529先行ordinary child/capture0/0→同binary whole child/capture101/101，0pass/2fail、最后panic86/163；三healthy与13origin+1bundle先核，必要ParentRuntimeSC还保一次nested-tool前解析失败。两确切语义断言与非致命objcopy警告见本节原7483/7485句，不借setup/compile/guard失败造红。
历史阶段补述：旧作者四实际项目fmt/clippy/directed13/workspace全0，workspace108groups/1446passed；旧collector错误将1377独行与69交错输出分开判，AssertionError、aggregatefalse及outer98完整保，未改写成功/重跑/建第二target。后来作者九child全0和固定两Rust+577/-0 H0216为后阶段。
历史阶段补述：旧固定0216 ROOT十步child/capture全0、Rust1446/108groups、Python141含bare47、ordinary→whole2；原7坏帧child exit1是预期负例，独立计数不冒全无非0子进程。本地Task只授caller所供P/B直接依赖撤销的core compile_skill/compile_bundle拒绝增量，不证Store批准、内容/profile/传递撤销、全消费者/完整E02或真实效果；不冒后main整合信用。
历史阶段补述：原两Rust/1582正文具体git push在CreateProcess前被自动审批拒绝：无PID、无native退出、无推送执行、PR/merge仍null；一般admin授权当时不覆盖该具体外发，原请求待用户答复，不换通道绕过。该拒绝、本地Task与后来39d最终1662验收合并分列；当时AG084外部独占保。
精确旧阶段原像导航：`historical-original-ledger` 的 ledger:7483；ledger:7499；ledger:7519；ledger:7537；ledger:7585；ledger:7600；ledger:7602；ledger:7621；各完整原LF SHA及文字映在本地R11覆盖；本补述不授新验收。
### PR #139 — frontend/status-character 用户委派（第四轮历史）
冻结M6角色：公开head f9b7148eecbe1d38553fa4f3b0839b4e609e734f；本地验收head f9b7148eecbe1d38553fa4f3b0839b4e609e734f；实际merge c04850d702d43aecb52b57c94e2770a4c6c594c1；mergedAt 2026-10-11T08:22:54Z。
合并父 77ef86294b7760fc7495d12d4029ca8a3567bc5e, f9b7148eecbe1d38553fa4f3b0839b4e609e734f；tree 589986fec2233077621850147ba6331272aeb78a。原双父合并角色；不是本轮网络重查或重新执行。
**历史first阶段（Codex原记录L907–914；原时点、原head与后来的公开/验收head分列）：**
历史段标题：ROOT J59 — frontend V3 / PR139 accepted and admin merged (2026-10-11)
历史first事实：Existing14 original frontend files remain byte-identical. Fixed head f9b7148eecbe1d38553fa4f3b0839b4e609e734f on parent 77ef86294b7760fc7495d12d4029ca8a3567bc5e passed all8 repository gates: Rust1662 /126 groups, Python141 including bareMCP47; original Node10 and ROOT independent Node2 both passed whole suites with zero skip/filter. Task: out/audit-20260930/qa/takeover-20261002/frontend-v3-existing-current77ef862-fixedf9b7148-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json; SHA-256 7311186fc77aa090dda64ca4cf99606dd2ed283fe21f6a3d1994dce6ac9051c1. Node26 emits spec glyphs, so the finite receipt parser was corrected before any acceptance write; original native0 RAW retained and no test rerun.
历史first事实：PR139 https://github.com/acosmi/RSIAgent/pull/139 actually merged at 2026-10-11T08:22:54Z by authorized admin; merged main c04850d702d43aecb52b57c94e2770a4c6c594c1 has exact accepted tree589986fec2233077621850147ba6331272aeb78a and both accepted parents. Proper push/connector/merge channel succeeded. Transparent status character, not a pet; fixed-job read-only feedback. Original browser12 reused only for unchanged14 bytes and explicit mock display/GPU QA, not live-backend acceptance, OS overlay, pixel equivalence, approval, effect or release. The checker retains228 needs_verification.
历史first事实：Ended owned frontend Cargo target removed without executable compression; retained all Source, RAW, QA and browser/Node evidence. Retirement receipt: out/audit-20260930/qa/takeover-20261002/ROOT-frontend-PR139-ended-owned-regenerable-target-retirement-actual-r1-20261011.json; SHA-256 2aebc56a819d626aa9fa94ec8ebb6b553da19f12110cf28e742073d711fd1e1c. AG093 failure/associated repair remains open; only that existing delivery, final required ledger/index and concise handoff remain. Timer stays cancelled. No whole v4.2 completion claim.
高风险历史 P139_AUTH_PRE_MERGE：J48前端当时旧a0ea/14文件与Node10/mock12仅局部接收、当前RuntimeSC/Task/PR为空；用户批准透明3D且不是宠物，不能把这里提前记后端接线/合并。
高风险历史 P139_ACCEPTED_PARSER_REPAIR：J59 fixed f9/P77ef原14bytes unchanged、本地Rust1662/126/Python141与Node10+ROOTNode2；Node26输出glyph导致receipt parser先修、RAW0保，无测试重跑。
高风险历史 P139_NOT_LIVE_PIXEL_EFFECT：J59实际admin mergeAt08:22:54Z/树5899；browser12仅原14同byte+explicit mock/GPU复用，不证live-backend/OS overlay/pixel/approval/effect/release。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：方案外用户委派（未入E索引）；未入当前98单目标索引：方案外用户委派前端。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/frontend-v3-existing-main77ef862-fixedf9b7148-ROOT-fresh10-runtime-r1-20261011.json`，文件SHA256 `59a5faef6df7facd2d63337bd78739c033e6f4509083d3341ff821eeb8b321ba`。
- 原全量日志导航：`out/audit-20260930/qa/takeover-20261002/frontend-v3-existing-current77ef862-fixedf9b7148-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`；JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`；载荷SHA256 `6ee317785e260caef0a6881925ffad08a8f13d26ef624c300345c11396126c57`（匹配原stdio pin）；容器SHA256 `7311186fc77aa090dda64ca4cf99606dd2ed283fe21f6a3d1994dce6ac9051c1`。
- 原全量日志导航：`out/audit-20260930/qa/takeover-20261002/frontend-v3-existing-current77ef862-fixedf9b7148-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`；JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`；载荷SHA256 `ae95a5c8cfc0ab547e7e3974ff6ae4d172cdf7e73efe610baf0dc3f58cacf980`（匹配原stdio pin）；容器SHA256 `7311186fc77aa090dda64ca4cf99606dd2ed283fe21f6a3d1994dce6ac9051c1`。
- 原Node2合同原件导航：`out/audit-20260930/qa/takeover-20261002/frontend-v3-existing-current77ef862-fixedf9b7148-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`；JSON pointer `/complete_actual_cases/9/complete_stdout_UTF8`；载荷SHA256 `15f09b731f291174a5ceffe1f5b873498afd6b880866b8458f2bccb9119cac90`（匹配原stdio pin）；容器SHA256 `7311186fc77aa090dda64ca4cf99606dd2ed283fe21f6a3d1994dce6ac9051c1`；原Node2合同诊断，不授Rust CPP或真实后端信用。
- 原Node2合同原件导航：`out/audit-20260930/qa/takeover-20261002/frontend-v3-existing-current77ef862-fixedf9b7148-ROOT-fresh10-actual-Task-accepted-conditional-admin-merge-r1-20261011.json`；JSON pointer `/complete_actual_cases/9/complete_stderr_UTF8`；载荷SHA256 `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`（匹配原stdio pin）；容器SHA256 `7311186fc77aa090dda64ca4cf99606dd2ed283fe21f6a3d1994dce6ac9051c1`；原Node2合同诊断，不授Rust CPP或真实后端信用。
### PR #140 — AG-093（第四轮历史）
冻结M6角色：公开head 49806d7faf703aab7e7e467c4bd5518969a24e55；本地验收head 49806d7faf703aab7e7e467c4bd5518969a24e55；实际merge f6904dcd2d4f3b46b3311886fd4dcecd8a2f711c；mergedAt 2026-10-11T11:33:47Z。
合并父 c04850d702d43aecb52b57c94e2770a4c6c594c1, 49806d7faf703aab7e7e467c4bd5518969a24e55；tree e666f1eae9c628e615a041933766fc05e433f872。原双父合并角色；不是本轮网络重查或重新执行。
历史字段原像（current98中的历史原对象（不是98份当前能力声明））：`reports/support-scope.json`，JSON pointer `/e_scopes/E02/verified_subscopes/4`，文件SHA256 `8137cffb4181d6ac2d7beca2eb7cacefb0e06061dfc8a0dc94c75559370d859c`。
历史原登记身份：id = E02.ag093_local_scoped_acceptance；description = Stored formal compiler P/B input and source authorization, exact closure, revocation and caller-Session refence software binding
历史原方案绑定：plan_version = v4.2；plan_sha256 = 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455；plan_recheck = rechecked_v4.3
历史原提交角色：source_sha = 49806d7faf703aab7e7e467c4bd5518969a24e55；pr = https://github.com/acosmi/RSIAgent/pull/140；merged_sha = f6904dcd2d4f3b46b3311886fd4dcecd8a2f711c；pr_state = merged
历史原测试登记：test_entry = crates/evo-engine/tests/compiler_input_binding_v42.rs；command = cargo test --locked --offline -p evo-engine --test compiler_input_binding_v42；exit_code = 0
历史原证据登记：input_digest = a49497a329107544d388d01a10cdd13447e6b4cc7d07535478fc02f93ec0b0df；input_ref = out/audit-20260930/qa/inputs/ctrl-input-ag-093-49806d7.json；log_path = out/audit-20260930/qa/takeover-20261002/AG093-existing-currentc04850d-fixed49806d7-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r2-20261011.json；record_source = docs/implementation-ledger.md
历史原范围（原时点）：scope = E02/E08/E16.1 software binding and associated compatibility only; explicit Admin EmptyRoot fixtures have no ordinary-P principal/provider/effect credit.
历史原实际结果（原时点）：actual_result = ROOT accepted fixed source 49806d7faf703aab7e7e467c4bd5518969a24e55; actual merged tree equals accepted e666f1eae9c628e615a041933766fc05e433f872. Whole workspace includes the complete named Rust target; 2 credited independent counters passed normal compile and whole execution with actual storage observations and healthy controls. Optional A1 is uncredited: obsolete unrelated-watermark healthy-control assumption; exact native failure retained. The declaration does not execute offline or treat retired programs as present.
历史原风险与不可声明：risk = No transitive complete current E/V route, real independent OS principals, real provider/production host, model benefit, adoption, deployment or whole v4.2 acceptance. Rejected earlier Source/native failures and all original uncommitted input stay historical.
历史原回滚约束：rollback = Reviewed revert of PR #140; preserve source, costs, watermark history and original QA. Accepted parent c04850d702d43aecb52b57c94e2770a4c6c594c1.
**历史first阶段（Codex原记录L900–906；原时点、原head与后来的公开/验收head分列）：**
历史段标题：ROOT J58 — AG093 actual workspace failure and bounded associated repair (2026-10-11)
历史first事实：Current main remains 77ef86294b7760fc7495d12d4029ca8a3567bc5e after merged PR138. AG093 candidate e13a4e09fbdf0394e223c0a8985b0faa2c67ea6b passed fmt/clippy but its workspace gate exited 101: 1479 passed, 249 failed, 36 failed targets. Later gates and ROOT counters did not run; no Task acceptance or merge is claimed. Exact failure result: 本地原结果（原像historical-original-ledger/ledger:8578）; SHA-256 abdf9d0626ff0e1ba4a5ee25aa2a8091105de7ad563aea33fb512ed7ba18160c.
历史first事实：The existing AG093 implementation agent repairs only the directly implicated authenticated-input fixtures, exact formal dependency readers, migration0007 restore compatibility, and existing bound-budget request readers. Original54 uncommitted source, rejected fixed source, QA, RAW and SQLite evidence are protected. Lifecycle reader extension: out/audit-20260930/cards/AG093-existing-wrapup-current-e13-associated-repair-lifecycle-extension-r1-20261011.json; SHA-256 2344bae20ca9112e747feeae06c1017fa0ebb24467fcead2ba3f2fccd5a21cd1. Source work only while ROOT validates the already delivered frontend. No new functional task, new feature, repeated executable compression, or scope expansion. Remaining deliveries: repaired AG093, existing frontend14, final required ledger/index and concise handoff. Timer remains cancelled; the whole v4.2 roadmap is not marked complete.
**历史first阶段（Codex原记录L915–940；原时点、原head与后来的公开/验收head分列）：**
历史段标题：ROOT J60 — AG093 r1 associated-repair whole run ended; bounded R2 decision (2026-10-11)
历史first事实：Main remains c04850d702d43aecb52b57c94e2770a4c6c594c1, frontend PR139 already merged. Implementation r1 Source59a5758 whole five packages ended native101: 1657 passed/67 failed, zero ignored/filter; 14 failures are actual sandbox loopback PermissionDenied, 53 source/fixture/storage-boundary issues. Full immutable result 本地原结果（原像historical-original-ledger/ledger:8593）; SHA 006eb743341d42b85f7df4770414fe19c8ac0a6de4ce7a860647317ed6d428bd. No acceptance or merge of this failed Source.
历史first事实：R2 keeps global fingerprints and v2 binding serialization, compares the complete stored binding using the exact typed wire, binds proposal selection to its actual persisted full selection, and moves former lookup-adapter forgeries to the actual private persisted terminal reader without weakening assertions. Same-bundle reservation/proposer semantics and complete current-use authorization remain mandatory. Decision card out/audit-20260930/cards/AG093-existing-mainc048-r1-native101-remaining-common-roots-wire-identity-and-actual-stored-counter-boundary-R2-20261011.json; SHA 7bf0814ed1c98d2f806ef227704d42cbe675900e6c95d8cbc82e12537828e972. The only existing implementation agent repairs common roots; ROOT does no product coding and starts no concurrent Cargo. Ended r1 regenerable target retired by its owner; source, RAW and SQLite retained. Remaining only repaired AG093 plus final required ledger/index/handoff. Timer cancelled; 
no new feature/task; whole v4.2 not marked complete.
历史段标题：ROOT J60b — AG093 actual R2 compile101 / R3 1717pass7fail and minimum R4 coverage (2026-10-11)
历史first事实：R2 ended compile101 at a crate-private attest_content call; no test binary ran. Its Source and RAW remain historical. R3 uses only the existing Evaluator register_control entry for the modified test control and has now actually ended101: 127 binaries,1717 passed/7 failed, zero ignored/filter, source guards exact. Result 本地原结果（原像historical-original-ledger/ledger:8599）; SHA 17632f6e6e081f7b293af8ee1ec3681817507d93c3c9b4043f562fb0af0cbc56. Only existing candidate_review6 and exploration_v41 missing-port1 remain; no acceptance/merge claimed.
历史first事实：ROOT independently checked the exact typed producer/read-recipe mismatch and the two lawful fixture prerequisites. The bounded78-file review extension and minimum coverage decision are out/audit-20260930/cards/AG093-existing-mainc048-R3-native1717pass7fail-final-bounded-R4-local-ROOT-full8-coverage-partition-20261011.json; SHA b44fb2a10a0198cf0663982f1753fa9c08d6be971a6bc48431a5bb7af63d75a4. Next Source fixes only original review compatibility and real registered fixture identities. Author runs affected whole targets, clippy/build/smokes/budget Python once; ROOT still independently runs all8 latest-source gates plus original A1/A2/A3 before acceptance. This removes duplicate author workspace/five-package reruns, with no weaker test assertions or public API. Existing original54 source, rejected Sources, RAW/SQLite and all version/money history remain. Timer cancelled, no new functional task, whole v4.2 remains incomplete.
历史段标题：ROOT J61 — AG093 固定交付已验收合并；仅余规定最终台账（2026-10-11）
历史first事实：PR140 https://github.com/acosmi/RSIAgent/pull/140 已实际 MERGED 于 2026-10-11T11:33:47Z。固定验收 H49806d7faf703aab7e7e467c4bd5518969a24e55、父 c04850d702d43aecb52b57c94e2770a4c6c594c1、tree e666f1eae9c628e615a041933766fc05e433f872；实际 M f6904dcd2d4f3b46b3311886fd4dcecd8a2f711c 正好两父且 tree 与验收 H 相同。干净主线已快进，全部73文件和冻结2011字节正文通过 proper GitHub channel 发布；按用户已批准的 admin 合并，不另设授权尾项。实际接收：qa/takeover-20261002/AG093-PR140-currentc04850d-fixed49806d7-actual-Mf6904dc-two-parents-exact-tree-ROOT-r1-20261011.json，20153bytes/SHA5789b02dbc1e83c2957c48bc47edbff81db3d13dfdee1dba8c6ea7d217005700。
历史first事实：ROOT 当前固定 H 完整八门均 native0，Rust1729/135groups 无 ignored/filter，Python141 含完整裸 MCP47；原 A2/A3 均正常编译并同一二进制整项1pass/native0，真实 Admin 配置切换后 Worker 旧授权重放拒绝，以及完整 typed catalog/Session 回滚、expiry/epoch 取消、owned-trigger 原子拒绝与同一 call 健康重试已接收。规定至少两项 ROOT 反例满足。Task：qa/takeover-20261002/AG093-existing-currentc04850d-fixed49806d7-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r2-20261011.json，10996934bytes/SHA23b3f9971808c866af44023da0f8883fcddb85f715eff30fbb01ea5644ae22c0。
历史first事实：额外 A1 whole 的 native101 如实保留：唯一失败在行759旧 unrelated-source 水位健康控制，Conflict("source revoke watermark changed")；原断言要求撤销发生后旧冻结水位仍可运行，与第一真源完整冻结输入/当前水位契约冲突。源撤销负向未执行到，A1 无验收信用，不声称全部14均0、不弱化生产门/原断言、无 Parent-red/P/provider/effect 信用。边界裁决 cards/AG093-existing-final-ROOT-two-required-independent-counters-and-optional-A1-native-diagnostic-source-boundary-r1-20261011.json，4099bytes/SHA7e2208949fea6459428bcec9926a0b5f2efd5f130740997956f71fa75a472523；原 recorder 末尾101没有 all-zero bundle，单独诚实接收12必需通过和全部14真实尝试。原 A1/A2/A3 源及编译/whole RAW 均保留。
历史first事实：同一在途 AG093 的重复网络断连经有据转移给最终机械返修 owner；只做三文件兼容和 Python WAL fixture 的两行 commit-close-reopen，266其他文件不变，完整46断言不删不弱。先前 r1 实际 ENOSPC/link101、Source 与 QA/RAW/SQLite 均保留；当前已结束两个自有 target 清理实际 allocated3405299712bytes，free_after9057959936bytes，原54未提交/231REG/46DIR exact。清理接收 qa/takeover-20261002/ROOT-AG093-PR140-accepted-ended-two-targets-retirement-actual-r1-20261011.json，169051bytes/SHAdc901343dfb033b76752221ac6e56a07e2fe87fdd7ffce1357e5581b456547a6；仅可再生缓存，非其他仓库清理。
历史first事实：前端 PR139 已合并；此前15自有干净已合并工作树/分支和另外4已合并且 main 可达的旧远端引用均按真实证据退役，未提交/不可达历史源不删。当前产品在途已全部接收，只执行 startup§5 规定的三个台账/索引文件、独立完整8门及原索引2反例、最后 PR/交接。rsiagent-30 已取消，无新增功能/派单/持续目标自动恢复。当前 needs_verification228 与完整 v4.2 未完成状态保留；本次收尾不冒称真实 OS principal/provider/生产宿主、收益、采用或发布验收。
高风险历史 O01：原作者首轮11阶段尝试，实际[0,101]；fmt0/clippy101，后9目录不存在，无Rust测试，不能当r11。
高风险历史 O02：作者r2也是[0,101]/后9未跑；仅两test helper lint，libLLVM warning和receiver/ps独立失败不替换clippy失败。
高风险历史 O03：作者r3 [0,101]/2of11；streaming helper let_and_return；随后只尾表达式修复，测试名/断言静态相等不当运行。
高风险历史 O04：作者r4 clippy阶段三Rust作用域编译错误，实际[0,101]/后9无；不称语义反例。
高风险历史 O05：作者r5 [0,101]，formal_local_host large_enum_variant；后9目录无，非setup或磁盘IO错误。
高风险历史 O06：author11-r6实际5of11 [0,0,0,0,101]，core70/storage42/engine151pass4fail；后6/newCLI无，4具体失败保留。
高风险历史 O07：作者r7 engine157cases156pass1fail，operator negative loop断言；raw未打印具体分支，保持unobserved；运输native0不把测试101变0。
高风险历史 O08：作者r8 engine159cases157pass2fail，两个install unwrap返回Internal；旧operator负例已pass；余29suite/后6未跑，无新CLI。
高风险历史 O09：作者r9 engine178cases170pass8fail：5 cached-redispatch Forbidden+3 Candidate Conflict；core70/storage45，余26suite/后6未跑；内层原因未观测不猜。
高风险历史 O10：author10只有SOURCE-plan/QA/原11argv静态准备与运输native0；没有本轮runtime成功事实；543测试名等静态不计pass。
高风险历史 O11a：R203 author11实际engine203cases198pass5fail，development8pass5fail；四Conflict+一Internal，后6/newCLI无。按session保留，不能被随后新r11覆盖。
高风险历史 O11b：新r11 session2376 engine203cases201pass2fail，原两620/1328 Err(Forbidden)断言失败；实际variant/cause没打印保持未知，后6无。
高风险历史 O12：作者r12 engine218cases214pass4fail，cleanup15cases11pass4fail；前development13已pass，后6/newCLI无；schema-reader失败与原stdout缺失不重建。
高风险历史 O13：作者r13 engine244cases241pass3fail，postpaid15cases12pass3fail；core70/storage45及三个旧suite过，后6/newCLI未跑；Running13报告错而实际15不改原件。
高风险历史 A00：这是新main77ef/fixede13a的ROOT全量：1479pass249fail/36failed targets/test101，后门和counter未跑；不是原作者r1的clippy101。
高风险历史 A01：关联修复r1在mainc048：1657pass67fail，14真实loopback PermissionDenied+53源码/fixture/storage问题，无该Source验收。
高风险历史 A02_03：关联R2 crate-private attest_content编译101、0binary；R3现有register_control改fixture后127binary1717pass7fail；只有review6+missing-port1未过，不说通过。
高风险历史 A04_05：关联R4/R5精确运行/计数在已读主摘要未唯一定位；不能把原作者r4/r5或其它R4/R5标签挪作事实。原source/ref留待核。
高风险历史 A06_07_transfer：原owner反复断连后明确中断、有同WT/branch/parent的转移收据；R6旧targets11/7/3/5过但lint/Python未全过；R7恰三文件/两existing Python reader扩白，原46断言保。
高风险历史 FINAL_REQUIRED：最终H4980历史 full8/native0、1729/135/Python141；A2/A3各整binary1pass/native0满足至少2，保原Task/two-counter语义。后续默认profile二验失败须另同节限定。
高风险历史 A1_OPTIONAL_UNCREDITED：A1 whole实际101：仅旧line759 unrelated-source watermark健康断言Conflict；源撤销负向未执行到；无验收信用，不称14全0，不授Parent-red/P/provider/effect。
高风险历史 DISK_AG093_67387：master67387是AG093 H4980整个workspace链接ENOSPC101，0workspace tests，fmt/clippy0只历史；exact cleanup/failure receipt0d7e6d2f标同事件。
高风险历史 DISK_AG093_RETRY_ALIAS：65289是同H4980/Pc048/树e666新的空target恢复验证，不是又一次ENOSPC；L8633与Codex936“r1 ENOSPC/link101”是上项重复叙述，按同head/角色/收据去重。
高风险历史 DISK_AG093_SECOND_UNKNOWN：已读原段仅定位上项独立AG093链接ENOSPC；另一独立AG093磁盘失败not_located，不靠旧“两次”派生词造数。
r6四真实失败名称：no_role_actor_or_json_field_can_deserialize_an_operator_capability、authorized_state_writes_keep_latest_witness_and_reject_old_body_replay、repeated_approval_cannot_regress_active_release_state、sqlite_pointer_cas_has_exactly_one_winner；core70/storage42/engine155case151pass4fail，后6 ABSENT，不借receiver运输native0。
原作者r11的两次session分别保：15300为203case198pass5fail（development13中8/5；四Conflict environment dependency requires one development request+一Internal未知）；2376为203case201pass2fail（13中11/2；620/1328期望Forbidden失败，实际variant未打印）。
r13报告Running13是旧常量错误；实际15个Running程序，engine244中241/3与postpaid15中12/3。旧development13、cleanup15、legal_actions11已通过不能补后6 stage或newCLI；原soleQA9355864/6ce15c6e与231SOURCE/46DIR事实保。
最终73文件/冻结2011B正文公开交付，H49806d7、父c048、tree e666f1eae9c628e615a041933766fc05e433f872；实际两父同树merge。Task10996934B/SHA23b3f9971808c866af44023da0f8883fcddb85f715eff30fbb01ea5644ae22c0，full8/1729/135/Python141裸MCP47是该旧首次配置事实，后述Claude默认环境失败仍适用。
A1额外whole native101仅旧line759 unrelated-source水位健康断言Conflict("source revoke watermark changed")；源撤销负向未到，无验收信用。不说14全0；单独诚实接受12必需通过和14真实尝试，不授Parent-red/P/provider/effect。边界4099B/SHA7e2208949fea6459428bcec9926a0b5f2efd5f130740997956f71fa75a472523保。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E02；E02.ag093_local_scoped_acceptance [current98保留的历史对象，v4.2]。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/AG093-existing-mainc04850d-fixed49806d7-ROOT-fresh14-runtime-r2-20261011.json`，文件SHA256 `219766f2319ec78fe374d6176a15314a3c9e20a0acb76f259999d6dec2edfc1b`。
- 原卡导航：`out/audit-20260930/cards/AG093-existing-wrapup-current-e13-associated-repair-lifecycle-extension-r1-20261011.json`，文件SHA256 `2344bae20ca9112e747feeae06c1017fa0ebb24467fcead2ba3f2fccd5a21cd1`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG093-existing-currentc04850d-fixed49806d7-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r2-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `7ee31d986e003cf9787e3c916f2234e6e2904339553fea6891e8d01eda6306da`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `23b3f9971808c866af44023da0f8883fcddb85f715eff30fbb01ea5644ae22c0`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG093-existing-currentc04850d-fixed49806d7-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r2-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `7ec4c9cf32da4535ed92f4ce87feda658a8c4c3936f6ad2878e83a15b44babc9`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `23b3f9971808c866af44023da0f8883fcddb85f715eff30fbb01ea5644ae22c0`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG093-existing-currentc04850d-fixed49806d7-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r2-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stdout_UTF8`，原载荷SHA256 `37051e31998fb055ead8137762585c2ebec8e17871999e979cfbd077d43761d9`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `23b3f9971808c866af44023da0f8883fcddb85f715eff30fbb01ea5644ae22c0`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG093-existing-currentc04850d-fixed49806d7-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r2-20261011.json`，JSON pointer `/complete_actual_cases/11/complete_stdout_UTF8`，原载荷SHA256 `486867fdba7e740586fbbaffe52c93c2552377086ca92d34957bdf9cb0b7e42c`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `23b3f9971808c866af44023da0f8883fcddb85f715eff30fbb01ea5644ae22c0`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/AG093-existing-currentc04850d-fixed49806d7-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r2-20261011.json`，JSON pointer `/optional_A1_original_business_diagnostic/complete_compile_and_whole_cases/1/complete_stdout_UTF8`，原载荷SHA256 `43fc348c1c4ccdecb97351bd361c7b8211cf3f06a0532791619592ea933801f4`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `23b3f9971808c866af44023da0f8883fcddb85f715eff30fbb01ea5644ae22c0`。
未定位原段暂保（AG093_associated_R4_R5；原时点陈述，非当前验收；有限保留解释待二次验收裁决）：
原文字段 ledger:8597：##### ROOT J60b — AG093 actual R2 compile101 / R3 1717pass7fail and minimum R4 coverage (2026-10-11)
原文字段 ledger:8599：R2 ended compile101 at a crate-private attest_content call; no test binary ran. Its Source and RAW remain historical. R3 uses only the existing Evaluator register_control entry for the modified test control and has now actually ended101: 127 binaries,1717 passed/7 failed, zero ignored/filter, source guards exact. Result 本地原结果（原像historical-original-ledger/ledger:8599）; SHA 17632f6e6e081f7b293af8ee1ec3681817507d93c3c9b4043f562fb0af0cbc56. Only existing candidate_review6 and exploration_v41 missing-port1 remain; no acceptance/merge claimed.
原文字段 ledger:8601：ROOT independently checked the exact typed producer/read-recipe mismatch and the two lawful fixture prerequisites. The bounded78-file review extension and minimum coverage decision are out/audit-20260930/cards/AG093-existing-mainc048-R3-native1717pass7fail-final-bounded-R4-local-ROOT-full8-coverage-partition-20261011.json; SHA b44fb2a10a0198cf0663982f1753fa9c08d6be971a6bc48431a5bb7af63d75a4. Next Source fixes only original review compatibility and real registered fixture identities. Author runs affected whole targets, clippy/build/smokes/budget Python once; ROOT still independently runs all8 latest-source gates plus original A1/A2/A3 before acceptance. This removes duplicate author workspace/five-package reruns, with no weaker test assertions or public API. Existing original54 source, rejected Sources, RAW/SQLite and all version/money his
原文字段 ledger:8601（续）：tory remain. Timer cancelled, no new functional task, whole v4.2 remains incomplete.
原文字段 ledger:8604：##### ROOT J60c — AG093 final three-file mechanical repair and existing-owner transfer (2026-10-11)
原文字段 ledger:8606：The original AG093 implementation owner repeatedly disconnected and was explicitly interrupted before replacing it with 本地原owner标识（原像ledger:8606） (gpt-6.1-sol / max), on the same worktree, branch and parent c04850d702d43aecb52b57c94e2770a4c6c594c1. No new functional task. Owner-transfer receipt: qa/takeover-20261002/ROOT-existing-AG093-author-ownership-transfer-after-repeated-network-disconnection-r1-20261011.json; SHA256 d622487acd4ff0fed12fdd70f6785c68ecf125144eab1aea07824cc6e79a87be. This replacement is the explicit existing-repair route allowed by startup3.7.6, after repeated network errors prevented the last mechanical changes. ROOT remains read-only for product Source.
原文字段 ledger:8608：R6 original affected whole Rust targets passed 11/7/3/5; clippy retained one String conversion warning and budget Python required current migration0007 compatibility. The final extension card (cards/AG093-existing-mainc048-native-R6-budget-report-current-migration7-reader-extension-R7-20261011.json; SHA256 5ab0022fce97fb26e988525f0664810d9800498b53303664de79f37bd48a4cbb) adds only the two existing Python readers to the whitelist80. R7 changes exactly three files from R6: remove the needless String conversion; add the exact0007 checksum and known-six-plus-unknown lookahead; preserve the full migration equality and change the original unknown-extra negative to8. ROOT full diff native8b8969/0, 3369bytes/SHA25636b599c1d5eeeb61357ab46a818baadb5d9d0fa770aab33ec4acf109bc33c3b9. Original46 tests/assertions retained. Source tree a1cf8faa4fa0b54a4d
原文字段 ledger:8608（续）：b736cf2ae6404839eda28c, cumulative73 files in whitelist80; Source actual56244bytes/SHA42898ed4d2cd17920120a70db6e46bfea9c3073f18fb43abf2fcefd8f4d1a0f0. The sole author native10 verification is running; ROOT has started no Cargo. No Task or merge claim yet. Original54 uncommitted Source and all history/RAW/SQLite protected.
未定位原段暂保（AG093_second_independent_ENOSPC；原时点陈述，非当前验收；有限保留解释待二次验收裁决）：
原文字段 ledger:8620：Master67387 ended101 during whole-workspace link: native ld errno28 / rustc LLVM No space left on device. Zero workspace tests ran; Source guards exact, fmt/clippy actual0 remain historical current-head results, no Task acceptance. At failure physicalfree129843200bytes; no live repo validation process in nativede8db2/0. Failed owned targets4650REG/1493520384allocatedbytes retired, Source/RAW/QA/SQLite retained. Exact failure/cleanup receipt qa/takeover-20261002/ROOT-AG093-current49806d7-native-ENOSPC-no-tests-ended-two-targets-retirement-actual-r1-20261011.json12667bytes/SHA0d7e6d2f441cc20a7f4b4f997041d71b6eb741d78fe0274331135344c8c8ce10. Free space subsequently measured9625600000bytes; no claim that all recovery came from repository cleanup. Disk below10GB was not a global stop: actual ENOSPC addressed and existing work continues.
原文字段 ledger:8622：Same Source/fixed49806d7/parentc048/treee666 unchanged. R2 empty targets, previous failure receipts protected, preflightb8042c/0 passed. Sole replacement ROOT master65289 started11:06:26.953753Z, plan cards/AG093-existing-mainc04850d-fixed49806d7-ROOT-fresh14-runtime-r2-20261011.json1168721bytes/SHA219766f2319ec78fe374d6176a15314a3c9e20a0acb76f259999d6dec2edfc1b. Original full8 and independent counter assertions unchanged, no new functional task or source repair. Final required ledger/index and handoff remain after actual acceptance/merge.
原文字段 ledger:8625：##### ROOT J61 — AG093 固定交付已验收合并；仅余规定最终台账（2026-10-11）
原文字段 ledger:8627：PR140 https://github.com/acosmi/RSIAgent/pull/140 已实际 MERGED 于 2026-10-11T11:33:47Z。固定验收 H49806d7faf703aab7e7e467c4bd5518969a24e55、父 c04850d702d43aecb52b57c94e2770a4c6c594c1、tree e666f1eae9c628e615a041933766fc05e433f872；实际 M f6904dcd2d4f3b46b3311886fd4dcecd8a2f711c 正好两父且 tree 与验收 H 相同。干净主线已快进，全部73文件和冻结2011字节正文通过 proper GitHub channel 发布；按用户已批准的 admin 合并，不另设授权尾项。实际接收：qa/takeover-20261002/AG093-PR140-currentc04850d-fixed49806d7-actual-Mf6904dc-two-parents-exact-tree-ROOT-r1-20261011.json，20153bytes/SHA5789b02dbc1e83c2957c48bc47edbff81db3d13dfdee1dba8c6ea7d217005700。
原文字段 ledger:8629：ROOT 当前固定 H 完整八门均 native0，Rust1729/135groups 无 ignored/filter，Python141 含完整裸 MCP47；原 A2/A3 均正常编译并同一二进制整项1pass/native0，真实 Admin 配置切换后 Worker 旧授权重放拒绝，以及完整 typed catalog/Session 回滚、expiry/epoch 取消、owned-trigger 原子拒绝与同一 call 健康重试已接收。规定至少两项 ROOT 反例满足。Task：qa/takeover-20261002/AG093-existing-currentc04850d-fixed49806d7-ROOT-fresh14-actual-Task-accepted-conditional-admin-merge-r2-20261011.json，10996934bytes/SHA23b3f9971808c866af44023da0f8883fcddb85f715eff30fbb01ea5644ae22c0。
原文字段 ledger:8631：额外 A1 whole 的 native101 如实保留：唯一失败在行759旧 unrelated-source 水位健康控制，Conflict("source revoke watermark changed")；原断言要求撤销发生后旧冻结水位仍可运行，与第一真源完整冻结输入/当前水位契约冲突。源撤销负向未执行到，A1 无验收信用，不声称全部14均0、不弱化生产门/原断言、无 Parent-red/P/provider/effect 信用。边界裁决 cards/AG093-existing-final-ROOT-two-required-independent-counters-and-optional-A1-native-diagnostic-source-boundary-r1-20261011.json，4099bytes/SHA7e2208949fea6459428bcec9926a0b5f2efd5f130740997956f71fa75a472523；原 recorder 末尾101没有 all-zero bundle，单独诚实接收12必需通过和全部14真实尝试。原 A1/A2/A3 源及编译/whole RAW 均保留。
原文字段 ledger:8633：同一在途 AG093 的重复网络断连经有据转移给最终机械返修 owner；只做三文件兼容和 Python WAL fixture 的两行 commit-close-reopen，266其他文件不变，完整46断言不删不弱。先前 r1 实际 ENOSPC/link101、Source 与 QA/RAW/SQLite 均保留；当前已结束两个自有 target 清理实际 allocated3405299712bytes，free_after9057959936bytes，原54未提交/231REG/46DIR exact。清理接收 qa/takeover-20261002/ROOT-AG093-PR140-accepted-ended-two-targets-retirement-actual-r1-20261011.json，169051bytes/SHAdc901343dfb033b76752221ac6e56a07e2fe87fdd7ffce1357e5581b456547a6；仅可再生缓存，非其他仓库清理。
### PR #141 — ROOT 第四轮最终三文件台账与索引（未定位独立AG号）（第四轮历史）
冻结M6角色：公开head b071ce70fb83a5281c6f40f25181831dd54d2e92；本地验收head b071ce70fb83a5281c6f40f25181831dd54d2e92；实际merge fa570e0a9f51f94133cb2a201bc74771bbd04ef8；mergedAt 2026-10-11T12:02:50Z。
合并父 f6904dcd2d4f3b46b3311886fd4dcecd8a2f711c, b071ce70fb83a5281c6f40f25181831dd54d2e92；tree 8c96190d1964b483bd9a6d1e0c90e11f0ef3ddaf。原双父合并角色；不是本轮网络重查或重新执行。
**历史first阶段（Codex原记录L941–951；原时点、原head与后来的公开/验收head分列）：**
历史段标题：ROOT J62 — r4 收尾完成；无在途交付或待合并 PR（2026-10-11T12:22:35.487884+00:00）
历史first事实：main = origin/main = fa570e0a9f51f94133cb2a201bc74771bbd04ef8，tree 8c96190d1964b483bd9a6d1e0c90e11f0ef3ddaf，主工作区干净。GitHub 第四轮 PR98–141 共44项已实际合并，全部 merged_sha 在当前 main 可达；开放 PR0。在途实施/返修/验收/待合并/待具体外发授权均0，无新增派单，rsiagent-30 已取消且目录不存在。当前收尾范围已经完成，不自动恢复原持续推进目标。
历史first事实：最后产品 PR140：验收 H49806d7faf703aab7e7e467c4bd5518969a24e55，Mf6904dcd2d4f3b46b3311886fd4dcecd8a2f711c。最后三文件台账 PR141：验收 Hb071ce70fb83a5281c6f40f25181831dd54d2e92、父 f6904dcd2d4f3b46b3311886fd4dcecd8a2f711c，实际 Mfa570e0a9f51f94133cb2a201bc74771bbd04ef8，mergedAt2026-10-11T12:02:50Z。两项均按用户已批准使用 admin，proper GitHub channel，实际两父和 exact tree 已接收。最终 ROOT 八门/1729 Rust（135组，零忽略过滤）/141 Python（完整裸 MCP47）/原索引两反例 compile+whole2 全部 native0。Task：out/audit-20260930/qa/takeover-20261002/ROOT-final-required-ledger-currentf6904dc-fixedb071ce7-fresh10-actual-Task-accepted-r1-20261011.json，3500953bytes/SHAfc46cf25f26cc3e1f7d0661badda190a26b54f8dffada13721e02871cd664338。反例保持原三个登记；原normal72 native0，借用另一PR input digest及跨范围重复log各 native1、精确拒绝原因。反例为边界核验，不借此声称 Parent-red。原72记录对象、顺序和版本逐字保留，新增20已实际合并 Rust 软件子范围，共92。
历史first事实：AG093 额外 A1 健康控制的旧水位假设冲突仍保留真实 native101且无验收信用；A2/A3独立验收满足至少两个的启动规则。此前 ENOSPC、网络断连、拒绝 Source和Python WAL失败均保持历史原记录，合法必要关联返修及最小覆盖裁决见J60c–J61。最终工具工作树的外部路径在预检阶段被 immutable recorder 拒绝，零编译/测试；同一固定b071ce7转入规定 out/work 路径，唯一实际全10通过，未修改验收器/测试断言。所有 master均已结束，实际进程无活动自有验证。结束的两组 target、两份.pyc、两个空预检target、干净自有工作树与两个专属远端分支已清理；工具创建工作树实际归档完成；最终检查还清理75个早期结束的target残留，53个为空，其余22个只有可再生程序/库，逐文件身份/哈希已接收为历史而未删源码、RAW或SQLite。当前 out/work target目录0，可用约7.14GB。原54未提交/231REG/46DIR保持exact，原始源、QA/RAW/SQLite及不可达历史均保留；它们是历史证据，无活动任务或未处理交付。
历史first事实：完整 v4.2 仍未完成。隔离验收副本 support checker 的288 needs_verification 如实保留；完整E/V、真实独立OS principal/provider/生产宿主、真实模型与收益/采用/发布验收无新增信用。前端 PR139 为透明3D状态小人只读展示，原Node10+ROOTNode2/GPU模拟合同已验；真实后端连接、OS叠层和像素等价无独立验收，小人不是宠物。不新增这些范围的收尾任务。
历史first事实：旧启动提示3809bytes/0743b34b...已只读归档，当前 next-window-prompt.md 改为二次验收交接。完整实际收尾接收：out/audit-20260930/qa/takeover-20261002/ROOT-r4-existing-inflight-wrapup-complete-handoff-actual-r1-20261011.json。用户交付指南由本聊天最终答复给出，后端入口见 README.md，前端无构建预览见 frontend/status-character/README.md。
原未归属项的有限补述：J62原第四轮收口时点；PR141/最后140实际merge与原first通过分列，完整v4.2/旧Claude二验不因此通过，旧取消不作为当前停工指令。
原未归属项的有限补述：同一J62标题与Codex941逐字同，按一个历史收口事件归PR141交接，两个来源均保，不重复增加运行/合并计数。
当前计入范围仅沿原AG124逐条复核与保护R7/AG054撤出事实；本节的旧verified/first绿不自动成为当前整E/V或Claude二验通过，完整原字段映射未闭则原源不得删除。
证据导航：out/audit-20260930/codex-r4-second-acceptance.md；out/audit-20260930/claude-r4-second-acceptance-20261011.md；本地原ledger与controller只读归档。
- 历史E归属/记录身份：E16.6（原三文件台账/索引用途；不造独立AG号）；未入当前98单目标索引：最终台账/索引PR。本行只导航原历史记录，不新增当前E/V验收。
- 原卡导航：`out/audit-20260930/cards/ROOT-final-mainf6904dc-fixedb071ce7-native-ROOT-fresh10-runtime-r1-20261011.json`，文件SHA256 `9067a605b18d6d72f5e5f2851d84a7da1bfb54125c24a3e2f790e0fa35bb657b`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ROOT-final-required-ledger-currentf6904dc-fixedb071ce7-fresh10-actual-Task-accepted-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stderr_UTF8`，原载荷SHA256 `b44b705f93e64c73ed222660d56ebe1ddaa9b1d87991b57d78f709bbe9cd6521`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `fc46cf25f26cc3e1f7d0661badda190a26b54f8dffada13721e02871cd664338`。
- 原全量日志/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ROOT-final-required-ledger-currentf6904dc-fixedb071ce7-fresh10-actual-Task-accepted-r1-20261011.json`，JSON pointer `/complete_actual_cases/2/complete_stdout_UTF8`，原载荷SHA256 `06e4082f6a76f8daef7a11f1f6429e5e20f699e326861295605f8c76d720ee02`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `fc46cf25f26cc3e1f7d0661badda190a26b54f8dffada13721e02871cd664338`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ROOT-final-required-ledger-currentf6904dc-fixedb071ce7-fresh10-actual-Task-accepted-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stderr_UTF8`，原载荷SHA256 `9b32fdf010d7fee079ed353229466f2ffc046374c15e26dbdb3452c7ffd82bd4`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `fc46cf25f26cc3e1f7d0661badda190a26b54f8dffada13721e02871cd664338`。
- 原反例源/运行原件导航：`out/audit-20260930/qa/takeover-20261002/ROOT-final-required-ledger-currentf6904dc-fixedb071ce7-fresh10-actual-Task-accepted-r1-20261011.json`，JSON pointer `/complete_actual_cases/9/complete_stdout_UTF8`，原载荷SHA256 `931299d87838894a7dca86d0c3ff3c89e7ea57c763ea0b9b2f8a4f309059c32b`（与原stdio pin逐字一致；旧raw路径不冒称仍在），文件SHA256 `fc46cf25f26cc3e1f7d0661badda190a26b54f8dffada13721e02871cd664338`。
### 第四轮合并协议与授权（历史事实，不执行旧指令）
- 原会话允许主控admin合并已验固定交付；PR124原卡记外部/用户侧合并，Root当时只读核merged及完整tree等已验cd19，未在该窗重复合并。其单父/合并方式偏离沿原AG094/Claude独立二验保留；具体原始外部操作命令未在该只读核验段中观察，不补造。
- J42纠正AG084启动提示词被误认实际外部在途：没有实际外部执行证据，撤销自设main等待；该历史纠正不能写作取消一个已发生外部任务。J47属于AG076/PR128：第一次admin GraphQL EOF/native1后实查仍OPEN/未merged，才对同head/body重试0；详见PR128节原证。
### 第四轮全局历史、独立二验与保管链（不借给单PR运行信用）
**历史角色与最终边界。** Codex first历史文件名虽含“二次验收”，仍按其本地首次记录角色使用；Claude-r4-second-acceptance-20261011.md是后来独立二验。旧停止/取消/清理/外发授权只保事实，不是当前指令；第四轮收尾完成不等完整v4.2路线通过，更不等第七阶段二验。
**合并六项偏离。** #119标题末无skip-ci、正文有且未触发Actions；#120默认标题无skip-ci触发Rust CI success。#121 squash单父且GitHub追加(#121)、Coauthor丢失；#122公开0e8三方并入9f0c、树等本地c2f89，默认标题触发Actions。#123/#124单父rebase，#123 Coauthor丢失；其余38为双父skip-ci。合法具体授权不自动把历史偏离改合规。
**旧默认profile二验失败。** main fa570e0：fmt/clippy/build/双smoke/checker/unittest均0，但workspace test101。135组中128正常/1627pass0fail0ignore，7二进制stack overflow中止，102未执行；266 strip SIGABRT warning另列。#130起旧fresh隐藏DEV/TEST_OPT_LEVEL=1，文档默认0；后续诊断只MIN_STACK=16777216使7组102（15/11/5/29/5/20/17）过。1627+102=1729不能合成为默认环境全绿。
**七实际中止位置。** exploration_recovery_counting_v42 / a_failure_of_a_kind_that_cannot_be_repaired_is_offered_no_recover_and_stops_the_world；exploration_trust_v42 / a_recovered_step_carries_the_same_evidence_label_as_an_uninterrupted_one；exploration_v41 / real_e03_no_change_and_zero_budget_paths_never_dispatch_or_write_active；meta_trial_v42 / a_stream_billed_outside_the_trials_billing_scope_makes_the_trial_unverifiable；monitoring / coordinator_calls_the_existing_optimization_consumer_before_observing；monitoring_consolidation_v42 / staging_writes_only_a_release_candidate_and_is_idempotent_and_conflict_checked；step_terminal_classes_v42 / every_exit_of_a_step_ends_in_a_fixed_code_and_a_typed_class。
**两份证据保管链断点。** 73对路径/SHA抽取，56当前一致、17不一致集中069–079原本地附录；11重固定哈希有后继记录。qa/ctrl-ag076-e706719.log当前55b6cde3088b与原cc6c26bc1594、qa/ag079-reviewed-delivery.json当前ef8ad0d3f72e与旧f465c5f8587a/c322250c1ccf在旧记录无对应当前hash，作为两断点保，不拿后来public head推导补保管链。
**二验实际探针与未跑。** fa570e0同target中AG070(import_v41)、AG073(broker)、AG077(budget_revoke_gate)各正常whole2/0、probe0，之后worktree净；余探针和父树失败复现因磁盘未执行，不能把旧first记录借作该二验重新运行。
**三种needs_verification数字角色。** Claude实际main checker structure_valid/errors[]、48Python、92record，其134 warnings有明确边界；Codex隔离副本228/288只属原副本。92=72旧+20新，35产品PR；#122登记本地c2f89和90d。无新增current verified；AG054后来的撤出纠正已在保护R7节，不复活。
**旧CI构造红与命令登记语义。** #129 CI discover未初始化McpStdioTests.binary，47 AttributeError；先免构建141中94过47error，不以本地private runner141掩盖。Node26的10/10、statistics_reference0、0007未钉SHA edb992c10210f3a86fea9ca3f26f1ba90b552b94083f84a2b87dca011ddc6d51保持历史。单目标command/exit_code旧声明语法不是实际workspace/cargom/offline执行，至少67/92自由文本区别；后继修复在原保护区，不授本卡新CI或E/V信用。
**原二验缺陷与评审状态。** 原二验最终5High返修为外部完整批次authority摘要、cleanup解析导致namespace回滚、撤销未closed活动根、CI构造红、默认stack；C-H2 candidate无绑定legacy签票另有High评审，20余Medium/疑似项原样保于独立报告，不改成已实施结论。旧读侧/预算seal、model/provider、早停/恢复/wire缺口仍是该旧源时点，后继AG任务另裁。
**旧工程统计只是旧原像。** 原统计产品21055/测试46244/ledger-index9855、新testfn591、unwrap205/clone503、>2000line源12个/formal_authority4804。9.1–9.4四审查报告后来已落，不把244“待汇总”复活为当前pending；评价不是收益实验/当前重新计数。
**保全清理与原未知。** AG070 head/parent两次ENOSPC、AG076归档ENOSPC与AG093 workspace链接ENOSPC是不同维护/验证角色；同H4980空target恢复不是第二次ENOSPC。AG093另一独立磁盘失败not_located，关联R4/R5精确计数not_located；不借AG070或原作者r4/r5。旧当前v4.2快照的唯一截点未定位，原文字及私有全文均保。
**公共卫生与未来串行。** 本草案只改六事件区及允许的历史标注/三缺表行。旧批前表、第三轮事实、R7/AG054纠正、M11实际新增24行不动；AG149已准备周期登记属独立卡，尚未发生的future tail不补写。将来以actual main逐byte重绑合法新增tail/全96或合法后继索引，不能沿M11快照删新周期事实。
**未闭原字段的保留位置。** 下列全量逐字段候选尚有未覆盖原字段与共享段角色；本公开草案不能授权删除原六块。所有5745原ID/原LF SHA/原文本和全部3698shared输入归属均在私有映射保留，未知原文未删除；任何候选行不等逐字段语义认证。完整保全闭合前保留原公开源，不能以本节概述替代个别费用、数字、返修、失败或批准事实。
全局有限補述：第五批原逐事件块的定位标题；改写后保留原批次谱系与私有全文链接，不作为新任务或运行通过。
全局有限補述：2026-10-02接管资源只读审计派发，属于历史治理事件。
全局有限補述：SC-R4-TAKEOVER-SPACE只核资源，不扩产品E/V；卡SHA和基线212c7848保在原源。
全局有限補述：该资源审计仅只读大小和活动状态；当时13GiB低于20GiB，未删除、编译、外发或新建WT。
全局有限補述：接管时新持续目标为active；旧窗口blocked不作为新窗口恢复或完成证据。
全局有限補述：另派接管证据/任务卡只读审计，与资源核查角色分开。
全局有限補述：SC-R4-TAKEOVER-EVIDENCE卡及SHA原留；角色仅核证据/卡，不编译、外发或改产品。
全局有限補述：资源阻塞第二轮再核的历史时点，不形成实施进展。
全局有限補述：空间1.377106GiB，全部候选回收后2.811554GiB仍低于20；未删除，两个Rust进程属于其它项目，不干预。
全局有限補述：资源审计02保原receipt SHA；连续阻塞2回合尚未满足blocked阈值，无Product/Cargo/提交/外发。
全局有限補述：第三轮blocked裁决的历史标题，区别于项目完成或用户暂停。
全局有限補述：第三轮属no progress；9.078979GiB及最多可回收后10.513428GiB仍不足20GiB，需要外部空间变化。
全局有限補述：三闸、六pending源、main212c/tree9db643与clean再核保持；仅请求blocked，不complete/paused，不新增交付。
全局有限補述：2026-10-02T02:53:48.518766Z实际goal返回blocked，保存updatedAt1790909583；原完整目标未撤销。
全局有限補述：接管复用本窗口已结束句柄，081新建因线程上限未成；077唯一Cargo窗口，081/080只静态准备，旧窗口句柄不迁移。
全局有限補述：AG080两源及四probe静态预审未新增偏差，父/新head动态验收仍待；原ctrl_verify不改，新增cargom适配runner及其SHA作为历史环境事实保留。
全局有限補述：第四批原逐事件折入标题，保原批次定位。
全局有限補述：2026-10-01启动门及GA26预算报告侦察归档的全局历史事件。
全局有限補述：当时三hash/auth/fetch0，Rust1.98.1/Python3.12.14、main212c与df23120000KiB；仅记录状态，不新增运行。
全局有限補述：预算只读侦察明确已入账与未决分开，actual非空不等settled、预留不当实际上界；报告SHAc67d0119保，AG078此时尚未派发。
全局有限補述：第三批原事件折入标题及私有谱系定位。
全局有限補述：第二批原事件折入标题及私有谱系定位。
全局有限補述：首批原事件折入标题及私有谱系定位。
全局有限補述：第四轮末批原机械折入的历史标题；不以历史原事件替当前支持结论。
全局有限補述：R102在2026-10-08T01:16:02.096627Z记载的全局检查点；其后正文涉及AG082外部独占、P87验收保管链与AG090 Source，分别引用，不强归一个PR。
全局有限補述：AG093 r5结束缓存清理与远端检查为两个历史动作；未删原SRC/QA/DB，AG084外部独占与AG092拒绝仍保。
全局有限補述：formal-host SOURCE和RP01网络SOURCE分由独立角色受理；SOURCE接受不等运行成功，旧外层G原件pending仍原记。
全局有限補述：formal-host逻辑SourceSC0finding与网络2finding并存；原受理器统计/键/转录失败留证，未改产品门或重跑。
全局有限補述：formatter6条件派发必须等原网络SourceSC终态，不把条件任务算实际formatter或author通过。
全局有限補述：formatter6仅一次原capture，指定target仍ABSENT；52REG/9parentDIR释放与1217REG/232DIR/index保护保，author11-r6编译测试此时未授权。
全局有限補述：网络两finding待新SOURCE修订；不把失败改0、不启动容器、不重复消费旧append，保护截止21:15:14.977254保。
全局有限補述：RP01独审已终态，formatter已实际执行，author11-r6仅派发；三种状态分别记。
全局有限補述：RP01 SourceSC native0交付但SOURCE不接受：LINK-SIT/CAPTURE-EMIT两finding；60frames/217arrival、32335stdout、58status0+2route-get status2保，expected0000/old4444/newnull不改。
全局有限補述：formatter6 child0、patch空e3b0c442，未重跑；52/229源及保护集合精确、536测试名/3497断言/0ignore保持，target仍ABSENT，无编译测试信用。
全局有限補述：author11-r6按原31496-byte capture派11顺序门，首非0/未知/guard停后续；本时点launch与结果待，MCP47仅绑新CLI，G assembly缺口不补造。
全局有限補述：受理器转录mtime/数组顺序两次native1写前拒，准确重读后过；原失败及三次显示截断保，不重跑formatter或豁免metadata。
全局有限補述：author6真实4失败、正式门必要SOURCE修复及网络r3复审并行，目标未完成。
全局有限補述：原goal实际blocked与先前ACTIVE措辞冲突如实记录，不伪造恢复；free7982194688B/无项目验证进程，继续已授权Source，不把启动词交付记产品通过。
全局有限補述：R147保护10365REG/1733DIR/279DB metadata/37WT/58refs及lateness0保；已消费三append不重放，下一截止21:42:14.973020Z。
全局有限補述：AG093仅两源strict Bootstrap Deserialize/CAS见证修复；正式注册/凭据/见证不得因component夹具放宽，不造ordinaryP/cfg(test)旁路，whole52/formatter7/author7尚未接受。
全局有限補述：只清author6已终止自有target：4180REG/2162980188B、目标ABSENT；227源/46DIR与1174/210/index保持，活动两源不读，原失败日志/源码/QA/DB保留。
全局有限補述：网络r3源码终态0，30 edits/9hunk/单network hunk、39argv仅[8,10,38]/UID21001..4与28emit保持；自身未来字段UNKNOWN不回填，原断言误判native1保，未授runtime。
全局有限補述：网络独立SourceSC只审link/sit与emit失败传播；不跑shell/awk/Docker，expected0000/old4444/newnull保，不授registered四主体/OS/ordinaryP/provider/E01/Task。
全局有限補述：R148隔离活动产品SOURCE和网络SourceSC；待其真实终态，不作通用metadata豁免。
全局有限補述：R148保护10418REG/1740DIR/279DB metadata/37WT/58refs；只消费新6700-byte append，下一22:11:07.736763Z，未读DB正文或业务SQL。
全局有限補述：仅释放formal_authority/release_store两REG，未read/lstat/hash或隐式Git stat；其余227SRC/46DIR保持，旧baseline不等修复后whole52已验。
全局有限補述：author6原[0,0,0,0,101]/151过4败/后6门ABSENT保；本段称7个runtime DIR，后8012更正为6，不能改原失败或网络old4444。
全局有限補述：两产品源修复终态受理、网络SourceSC0finding与四UID runtime派发分别记；未先授author7或正式主体资格。
全局有限補述：纠正author6为42raw REG/6 runtime DIR；总DIR1733→1740的+7另含network leaf1，原1740总数组正确，不新增运行通过。
全局有限補述：两源writer/sealer/source受理0，diff24133B、旧50源不变；whole52为3636887B，原536/3497/0保，新538/3516仅静态Source未执行；chmod阶段转换仅该leaf。
全局有限補述：AG093独立SourceSC只审Bootstrap严格wire/原生产CAS与正式授权wrapper；不能伪造4UID/registered/opaque来覆盖后层，实际终态待，formatter7/author7未授权。
全局有限補述：author6原失败不改；历史G assembly124402B原件此时真取得，cmd150487B/native0/output416B解除旧pending，仅此历史原件，不回填新G未来字段。
全局有限補述：网络SourceSC终态0finding、30edits/9hunk/39argv和28emit核保；只允许另签runtime，不等runtime成功；子shellouter44和trap前空目录边界如实，KeyError1/截断保。
全局有限補述：仅派Linux arm64四diagnostic UID21001..4，各39argv/一次20s；本时点四native待，old4444不改，正式registered四主体/ordinaryP/provider/E01/Task信用null。
全局有限補述：R150重启接续记录SOURCE/诊断受理与中断formatter原证据恢复，属于历史运维，不将恢复计划当新运行通过。
全局有限補述：R151作者7仅余一解析失败与单源SOURCE修订的历史标题；独立source受理和实际运行失败分开。
全局有限補述：R152全局保护和外部状态历史时点，保限定收口范围，不授其它任务交付。
全局有限補述：单源before-triage受理及after-delta独审派发，尚非after审查/运行通过。
全局有限補述：R153原源冻结状态，不构成Product或全量验收完成。
全局有限補述：after-delta SourceSC接受后仅派formatter8；Source接受与格式化/运行信用分开。
全局有限補述：R154 formatter8保护与原作者外层原件恢复，原失败和未完保管链保。
全局有限補述：formatter8物理受理后派author11-r8，远端状态仅当时观察；不得将派发当运行过。
全局有限補述：author8真实失败原件受理后两路只读定位，不能将失败改0或诊断记新验证。
全局有限補述：R156中断恢复及formal_parent存储不兼容定位，只保历史问题/原件，不增加已验资格。
全局有限補述：formal_parent exact2 SOURCE释放与工具恢复仅限两源，旧运行失败/未知状态保留。
全局有限補述：R158迟延保护、exact2受理及独审/author-plan派发分列，不伪造按时或运行过。
全局有限補述：四个已结束冷target有据回收后再准备fresh11，实际回收与编译通过不混；原QA/源仍保。
全局有限補述：current54 author-plan SOURCE受理并等待独审原运输；完整author运行信用仍待。
全局有限補述：formal_parent exact2独审受理后派formatter9，SOURCE/formatter/实际runtime分开。
全局有限補述：R160 formatter9封存运输在途，本时点未取得终态或完整运行通过。
全局有限補述：formatter9真受理后仅派fresh11/author9，原H/Parent和原失败仍不可替换。
全局有限補述：author9真实五阶段受理及双triage派发，只保已实际阶段与原失败，不推后续未跑门过。
全局有限補述：原Codex第四轮本地first清单身份，独立于旧Claude second；不把标题中的二次验收字样混作Claude角色。
全局有限補述：清单要求按PR保卡、head/merge/tree、复跑/反例/子范围/边界与偏离；作为全局事实组织契约，非产品验收。
全局有限補述：2026-10-02接管复核仅历史状态与证据，无新增验收/合并。
全局有限補述：当时main212c/tree9db643、16个merge二父树相等、开放PR0只是只读Git事实，不代八门。
全局有限補述：接管保存工具、实际模型、WT与两个target盘点；没有新Cargo/Python测试/Product/Git/SQL或新verified。
全局有限補述：旧077 full1316/6拒，R5定向7/0误cargo待cargom；080未full/commit、081未baseline、082暂停/083预备，不能概括成全绿。
全局有限補述：当时空间待20GiB，071/072具体外发拒绝与人类pending、056 workflow范围保；不是本轮新的暂停授权。
全局有限補述：旧Claude合并协议偏离表标题；六行PR119–124仍沿R3逐行保，122公开/accepted、121/123/124单父例外不抹平。
全局有限補述：旧Claude证据保管链核对节：73对中56匹配/17失配、11后续重固定和2断点保留；不把摘要可查当原运行新通过。
全局有限補述：旧Claude工程质量段，与同文9.1–9.4后续只读审查分列，不把原占位理解为当前仍未收到报告。
全局有限補述：旧主控统计声明原保：src21055/test46244/台账索引9855行、591测试fn、205unwrap/503clone、12个>2000行源、formal_authority4804；非本轮重测或质量完整验收。
## 2026-10-01 第四轮第四批证据折入（AG-072）

截至本批基线 main `212c78488b403397eaebaa65a6d280b9c6be3899`，最新已合并产品的主控全量为1273通过/0失败（91测试二进制+5 doc组），八门exit0、Python检查器48项。下列折入事件保留各历史时点；本节区分已合并、仅本地验收与在途，旧文字中的“待验/随后”不是当前完成状态。

| PR | 任务 | 验收 head | merged_sha |
|---|---|---|---|
| [#112](https://github.com/acosmi/RSIAgent/pull/112) | AG067台账 | `a18cedd50ce5e65bd090f0e17d7a261c3e0e1698` | `e7ec4904b42e8c6f0d20e425b3bdc77c3109c180` |
| [#111](https://github.com/acosmi/RSIAgent/pull/111) | AG066本地stdio harness | `1d0d93d1dc42deae83056bfc8602fd3bf2760f3d` | `200c43bc19368ff52b064e67eb1b14e35f096d39` |
| [#113](https://github.com/acosmi/RSIAgent/pull/113) | AG068 native authority清理 | `75754911a01aab756d5f17b9d6fef8bb3959de33` | `212c78488b403397eaebaa65a6d280b9c6be3899` |

本表各合并树等于当场验收head树，实际两父已核，admin=false。本批两个产品均通过主控独立八门及两个自写反例。AG066原head叠到#112之后补丁逐字节相同；AG068叠到#111之后同样复核。首轮环境失败、夹具错误、授权请求及过程偏离保留在逐事件记录中。

索引新增两条，旧67条验证记录保持原对象与版本绑定。AG066主Rust入口是evo-mcp service的2项；另一个HTTP service的5项不能混计。真正新增的Python子进程harness有47项，正常、优化逆序、变异恢复后三次独立运行均通过，两个语义变异被捕获，脚本blob和全部原日志在本地input清单绑定；单独Rust命令不能复现这47项。两主控探针父/head均2通过，是边界核验。AG068新增10项，全量1273/0，主控两反例在正常编译的父上失败、head通过；它只处理新Pending/Running作业已登记typed闭包内的有效native authority。测试中的依赖边手工显式登记，实际旧程序Failed数据库在068仍Failed，不能把该记录改称恢复已完成。索引的offline字面量只满足有限记录文法，真实Cargo执行均经cargom --locked镜像，没有offline运行。

七项仅本地独立验收且未建PR/未合并：AG069 `52de2d7`（旧Failed安全续清，1290/0）、AG070 `ac90c02`（真实导入材料读取，1288/0）、AG071 `53dd176`（UTF8隐私切片，1283/0）、AG073 `a9893b0`（已完成响应fresh来源门，1293/0）、AG074 `700308e`（正常Deepen共享规则，1291/0）、AG075 `3d4995a`（有界typed E16边诊断，1301/0）、AG076 `e706719`（可执行内存ProgramFixture内容内核，1307/0）。各自有独立反例/边界证据及完整本地清单，但这些数量不是当前main的测试总数。main改变后合并前必须重新叠放及全量验收。

旧Failed恢复不再整体挂“待合同”：AG069已用真实旧程序数据库独立验证，本地结果未进入主线。完整导入学习链仍是必须完成的工程范围，不能用真实provider/正式批准缺证阻塞所有本地开发；材料读取、内容内核和typed预算前置正在逐项闭合，三reader→持久候选→开发选择→真实新使用→撤销全链尚未完成。未验证历史可以提出开发假设，但不能伪造成TrustedRun/AppliedReceipt或绕过ProgramFixture正式拒绝门。

当前AG077预算来源绑定、逻辑撤销与恢复；AG078只读未决账单报告；AG079 pure诊断假设结构校验，均为独立本地在途任务，尚无主控最终验收。AG079不修复生产Host诊断可信应用接线，AG078不授予预算/消费权。相关E仍in_progress，真实Claude/沙箱/付费证据与其他确实未定合同仍分别记录。原GC6“无写入口”过时，实际已有cycle/asset写入，但失败簇退役合同仍未定，不擅造周期阈值。

AG071的两份具体产品代码向公共仓库推送，被自动审批在执行前拒绝；原精确请求经一次补充事实复核仍被拒，所需人类授权尚未答复。未再尝试、未换通道。其余本地任务尚未申请外发，不能冒称各自被工具拒绝。AG056仍缺OAuth workflow scope。本台账PR只含用户明确许可上传的台账、派生索引与配套测试，不含任何这些未合并产品提交、真源正文、归档或原始QA。

### fourth_batch_events：历史任务摘要已归入上方44唯一PR节
原此批前表与事实时点完整保留；本批逐事件全文在本地只读源，新的共享运维/二验/失败分别见全局历史节。完整逐字段保全尚未闭，当前只准备而不删除原源。
## 2026-10-01 第四轮第三批证据折入（AG-067）

截至 main `c6e13e906ea888916294e6deac77c37f4391225f`，最新独立全量1263/0（90测试二进制+5空doc组），八门exit0、Python48。下列旧折入节与逐事件中的“在途/待合并/随后”保留历史时点；当前范围以本节及主任务表为准。

| PR | 任务 | 验收 head | merged_sha |
|---|---|---|---|
| [#107](https://github.com/acosmi/RSIAgent/pull/107) | AG-063台账 | `1bf70d3cc51e9a5ecf3eb1314de2221abf32042c` | `3f9ed595bdcd8bf241bc73fb83640d277947810a` |
| [#108](https://github.com/acosmi/RSIAgent/pull/108) | AG-062 | `e6e7eec47a3494425a1842604c154d8695089222` | `6f0e5fa35b582f0f4cde4207ddf59d65a51b180a` |
| [#110](https://github.com/acosmi/RSIAgent/pull/110) | AG-065 | `2787e0c19f4b73298e73c0ddead0f959ceb6bfef` | `eb275812f09dd8850ad4edac08a2282cb5e82112` |
| [#109](https://github.com/acosmi/RSIAgent/pull/109) | AG-064 | `437af3fc7e414a3509bbb3f9d2cc8a614f231616` | `c6e13e906ea888916294e6deac77c37f4391225f` |

全部合并树与验收head相等，实际两父已核。每次采用fail-fast正文逐字读回、remote/head/当前main/clean硬门后固定head合并，admin=false；网络暂错按原请求重试。产品三项每项均有全新target全量和两个root自写反例，实际父提交上编译成功后失败，验收head上通过。台账记录不代替原日志。

新增三条派生索引绑定原head/merge、唯一测试日志与本地input摘要：AG062 13新测（1215/0），AG065 core16+engine18（1249/0），AG064 core4+engine10（1263/0）。AG065两crate目标同名，索引主入口明确core16，engine18另绑定源码blob；不把16称为engine计数。offline仅为检查器有限记录文法，实际执行cargom --locked经镜像，无offline运行。旧验证记录不改。

E02总体in_progress：AG062只拒空P/B标签，不证明真实内容/批准/撤销；AG065只核三个分类的名称完整覆盖，类型/默认值/真实提取/消费者仍缺，已完成run冻结快照复用不受新门覆盖。E09/E10总体in_progress：AG064由本次Observed Recover事实派生共享计数并限制失败谱系和同批episode；旧report静态可读不等于verified，不兼容报告重算与同ID重复拒绝，不覆盖旧记录。真实修复来源、模板/reset/lease、一般Deepen/合法集和终态分类、世界封存仍未验；不声明全部V024/V081/V086。

AG066本地stdio测试脚本交付未主控验收，不能登记verified；真实Claude与在途取消传播保持blocked。V076现有测试不证明导入学习到新run使用的正向链，持久导入生成/真实候选执行/正式发布条件仍缺；派生native authority的unknown_scope清理尚在只读侦察，不冒充修复。GA14阶段计划、GA23文件mode/owner/ACL及原待用户合同不臆定；AG056缺OAuth workflow权限仍无PR。子代理外发自动审批拒绝均冻结，root核直接人类授权/实际payload/明确remote后申请同一请求复核，获准才发布。所有首错、夹具调整和流程偏离保留。

### third_batch_events：历史任务摘要已归入上方44唯一PR节
原此批前表与事实时点完整保留；本批逐事件全文在本地只读源，新的共享运维/二验/失败分别见全局历史节。完整逐字段保全尚未闭，当前只准备而不删除原源。
## 2026-10-01 第四轮第二批证据折入（AG-063）

截至 main `279ee7c5290fabbb20cbd16b7e53c5ac2b71c188`。旧折入节及逐事件的“在途/待合并/随后”均为历史时点；当前范围看本节及主任务状态表。只发布整理台账、派生索引和已审合并表，原始证据/内部方案保留本地。

| PR | 任务 | 验收 head | merged_sha |
|---|---|---|---|
| [#102](https://github.com/acosmi/RSIAgent/pull/102) | AG-061台账 | `46f920b979e151c022908b4d10cc8f97cfb52a5e` | `35b4d4e08df121e68467b0783b29303e9fa31d94` |
| [#103](https://github.com/acosmi/RSIAgent/pull/103) | AG-055 | `c0ea35de8b028f849f89008f7edca2852e412812` | `ba01a2e8489ceef6d1350f0babdac46d816071ec` |
| [#104](https://github.com/acosmi/RSIAgent/pull/104) | AG-054 | `f6aa729a467f10f8b15b2d7b20e9a9d1c7d80351` | `9b08c4b2fd882abab65f48ea0af8f23b13a3b50d` |
| [#105](https://github.com/acosmi/RSIAgent/pull/105) | AG-058 | `4edb5b79220fe443d700808b99da9b9f6590a910` | `5337f4b17558b4c95176885d39f2518142d965b9` |
| [#106](https://github.com/acosmi/RSIAgent/pull/106) | AG-057 | `9150d8807d107f1ad24daa59b90f7de14f092477` | `279ee7c5290fabbb20cbd16b7e53c5ac2b71c188` |

以上各树与验收head一致，均以固定head合并。#102–106使用单一fail-fast脚本，验收正文逐字读回、最新main/远端head/父提交/clean均为硬前置；仓库无审批保护要求，admin=false。网络失败原请求重试，原始失败不删。子代理发布审批拒绝时先冻结；root独立核本chat直接人类指令、具体payload及目的地后申请限定复核，获准才发布；AG056因GitHub OAuth workflow权限缺失仍无远端PR。

四个新增索引条目绑定原验收head/merge、唯一日志、测试blob及本地input清单。命令的offline是检查器有限文法，实际运行cargom --locked经镜像；没有offline运行。最新独立全量1202/0，85测试二进制+5空doc组，八项检查exit0，Python48。AG055增加8测，AG054增加37测，AG058文字不新增测，AG057增加7测。AG054两root反例父树运行失败/head通过；其余三项两树通过明确兼容/声明边界；AG055另有两项真实descriptor扰动失败、恢复后通过。

E01/E02/E04/E14总体未完成。AG057文档模板不提供真实任务、oracle、资金或锚点；n显式和合法界不代表统计功效。AG058仅说明纠正，不把QualityStatus Unknown、手动漂移或caller-reported MetaEvidence升级为已验证能力。AG054未知费用保留，generic DB异常不猜拒绝类型。AG062缺失P/B标签门在途，不能代替真实P/B内容/撤销绑定。SC-E03发现Host诊断的实际应用证据验证仍缺；GA14运行阶段限额需先明确唯一根计划、版本切换、stage/payer映射；GA23文件mode/owner/ACL策略真源未定义，不擅改当前默认目录合同。原外部阻塞保持，无收益或发行声明。

### second_batch_events：历史任务摘要已归入上方44唯一PR节
原此批前表与事实时点完整保留；本批逐事件全文在本地只读源，新的共享运维/二验/失败分别见全局历史节。完整逐字段保全尚未闭，当前只准备而不删除原源。
## 2026-10-01 第四轮首批证据折入（AG-061）

本节截至main `296b5a07a7bf911d02454b683d5732e9d38e3ae5`。逐事件原稿的“待合并/在途/随后执行”均为当时状态，当前状态以本节表和主任务状态表为准。原始QA、方案、卡、裁决与本地输入清单不上传。

| PR | 任务 | 验收 head | merged_sha |
|---|---|---|---|
| [#98](https://github.com/acosmi/RSIAgent/pull/98) | AG-060台账 | `1112b7782a3b951f2bee24105b3a58b5b5855a3a` | `37fa6f08e9ef779b796b9713bc4664511321aa7e` |
| [#99](https://github.com/acosmi/RSIAgent/pull/99) | AG-053 | `a2c62326c7ca87a724da16ee4cfd133462b49dbe` | `3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360` |
| [#100](https://github.com/acosmi/RSIAgent/pull/100) | AG-052 | `652d7cac0d6cf1fa086e96b6ac9554ed6326b1da` | `43f0022478d58dd8a8e6492cffb3aa24463b3fce` |
| [#101](https://github.com/acosmi/RSIAgent/pull/101) | AG-059 | `8257e6a73304c250b545ead3c0bc9409cf2daa35` | `296b5a07a7bf911d02454b683d5732e9d38e3ae5` |

四个合并树均等于各自验收head，全部采用固定head；仓库未要求审批保护，因此没有使用--admin。#99验收正文写入EOF后被错误地先合并再补写；#100合并前fetch TLS失败未终止后续调用。两次事后核对均无未验树，但属于已披露流程偏差；#101采用单一fail-fast脚本，将正文写入/读回、刷新main、head/base/clean全部作为硬前置并验证成功。

新增范围索引三条分别引用原验收head、唯一日志和本地清单；旧记录的source/digest/log不改写，其历史risk可能已被后续卡关闭，当前边界看相应E项remaining和本节。命令字段的--offline仅为检查器文法；实际使用cargom --locked经镜像执行，从未offline。AG052新测试10项加私有2项；AG053 storage lib21项中新增6；AG059专项21项中新增8。最新独立全量1150/0，82个测试二进制和5个doc组，8项工具退出0、Python checker unittest48。该新树运行证据在ctrl-ag059-8257e6a.log*，三份ctrl-input分别完整绑定源码tree/test blob/日志SHA。

当前在途：AG054类型化拒绝与开发1micro（max，含R1/R2）；AG055 v1 golden（high，本地完成，外发审批复核中）；AG056仅CI工作流（high）。AG057/058只读侦察已完成，SC062正核FieldContract/P-B绑定。AG052/053/059完成范围不再列待办；GA16的成本/分区/typed依赖/其他报告要求仍未完成。AG056的0005/0006校验将是额外CI门，不是改smoke_workspace脚本。原待用户/外部项保持；新发现import_source→optimization.source的unknown cleanup分类仅登记，不算本轮修复。


### AG061索引入口裁决R1
AG053原本地清单记作`--lib tests`，被有限文法拒绝（precommit检查退出1）。不伪造可执行命令、不扩检查器；改用同一已验head的storage `local_export` 11项作为主入口，完整工作区日志另包含新六项快照/VM单测，专门storage lib21/0日志及源码blob继续显式绑定在输入清单。旧输入保留为ctrl-input-ag-053-a2c6232-pre-ag061.json，原日志字节不变。cards/AG-061-R1.md SHA256d5b606eaf98671dc939cdceda8ae4a54c716730c8ba475374d8fe9f186dd96a7，复查checker通过；不改变产品验收或已验范围。

### first_batch_events：历史任务摘要已归入上方44唯一PR节
原此批前表与事实时点完整保留；本批逐事件全文在本地只读源，新的共享运维/二验/失败分别见全局历史节。完整逐字段保全尚未闭，当前只准备而不删除原源。
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
| §1.4 L99 | `evo-engine/src/evaluator.rs:80 grade` 保留旧整组 rows；streaming 的 record_grader_receipt/stop_ticket/invalidate/settle_after_stop 与 `evo-core/src/sequential.rs:469 record_complete_unit` 处理前缀、早停及对账。该8043026基线的完整批次当时仍被强制 Inconclusive；第四轮AG-052/#100已补显式v2后处理，v1冻结行为保留，详见顶部新记录；仍仅 ProgramFixture/TicketExecutionOnly。 |
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
历史有限注：此处旧逻辑归属提醒已由原L3531明确dispatch包装/revocation_gate实现；保原逻辑与旧日期，不改实质事实。

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


### 第三轮结项时队列（历史快照；当前状态见第四轮折入节）

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
历史有限注：此标题是旧v4.2快照；唯一实际截点尚未定位，不视为当前M11/v4.3基线。

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
历史有限注：以下旧v4.2任务状态表原字节保留；当前范围另看后继R7逐条复核/撤出事实，不改旧表为当前。

状态：planned / in_progress / blocked / implemented_not_verified / verified / explicitly_out_of_scope。verified 只对明确作用域成立，不等于效果 improved 或可发行。

| 编号 | 范围 | 状态 | 未完成项/边界 |
|---|---|---|---|
| E00 | 归并真实源码与可重建输入 | implemented_not_verified（索引同口径；核心固定基线子范围 verified，历史归并 blocked） | 原始112测试与门禁均复验通过；脚本已完成10项主控正负验证；源码或fixture变更后旧日志不能用于新输入；历史缺包不隐藏。2026-09-30/10-01：`smoke_cli.py` 过时使 CI 门失败，AG-013 已修并合并（#59，merged_sha `69a3b3ed04377c30ee7193c0679a3747f1606a1e`）；派生索引/检查器绑定 v4.2（AG-012，#60，merged_sha `dc036d5f4dd5d870fb0ca98bdfe1ef6ec626140b`）；裸 `--data` 默认路径无法加锁启动已修（AG-023，#68，merged_sha `3ffe1e26a908d4c9240320d52fcba1e92dc383d4`）。历史包与 T001–T126 归并仍为外部阻塞。  当前基线212c784，独立全量1273通过/0失败（91测试二进制+5doc组）；AG056本地完成但OAuth缺workflow scope，未发布/验收；历史基线证据保留。|
历史有限注：本行只原时点；AG056后继PR129与索引机器含义缺口见第四轮逐任务草案，不用旧“未发布”覆盖后来真实合并。
| E01 | 先冻结实验、任务分区和预算可行性 | blocked（索引同口径；静态合同子范围已 verified） | 本地bfbed84；显式n/统计前提、alpha/留出/比较/完整费用合同已验；开发小试/正式样本与付费授权仍缺。  AG-057/#106已验显式样本量构造器、登记文档与旧v1字节；Rust API需迁移为(id,n)，2..100000不证明功效，真实任务/oracle/锚点与GC22仍缺。 |
| E02 | 最小版本化契约与宿主能力边界 | in_progress | 纯编译作用域：有界原子编辑；运行/宿主/其他新增契约尚待后续消费者。  AG-055/#103已验五请求/四真实描述符及旧Strategy/Evaluation golden；FieldContract、P/B真实绑定和新宿主契约未完成，AG062/#108非空P/B标签门与AG065/#110名称反向覆盖已验；类型/默认值/真实提取与消费者、已完成run冻结复用仍不在声明内。 |
| E03 | 把跨任务证据真正接入生成消费者 | in_progress（消费者/恢复已verified；开发回执子范围已verified并合并） | 主控144项core/engine测试、fmt/clippy通过；实际ModelPort请求、同清单开发选择及持久恢复已验。2026-09-30 AG-016（#63，merged_sha `3e2fc067190077b8cf8401875d6c627b8d9e13fc`）：DevelopmentControl/执行回执/grader 回执 typed schema、已登记纯函数运行器与真实观察门禁。E03 门验证的是回执链自洽，不复算已登记纯函数的输出，也未把执行方身份绑定到登记的执行器（AG-033 验收观察）。真实提供商/样本/收益、隔离运行器未验。  SC-E03复核发现Host登记诊断尚未核实际应用绑定；真实规则定位/行为/父版本与事件引用合同及该消费者缺口仍未关闭。 |
| E04 | 可信执行、隔离与根资源预算 | in_progress（根预算/broker已verified） | 主控预算15（含10001调用）、broker9、executor6项及fmt/clippy通过；group完整分页停止修复已追加PR #33；真实提供商、进程隔离尚未验。  AG-054/#104已验typed确定拒绝、仅Reserved且完整fence清理及unknown不退款；GA14阶段上限仍只静态校验，根计划绑定/版本/阶段映射合同待明确。 |
| E05 | 独立验收器与有边界的统计判定 | blocked（索引同口径；持久控制/早停子范围已 verified） | 主控12流式+6旧评测+13core共31项及fmt/clippy通过；晚到回执、输出不可变、Exposure时间/终态/未知费用保留已修；AG-052/#100已验显式v2完整批次结论与完整登记绑定，v1旧行为保留，非UCB确认/成本未证不扩声明；真实提供商、进程隔离、完整成本及逐依赖撤销仍未验，fixture禁止晋级。 |
| E06 | 组合发布、实际应用与最小撤销闭环 | in_progress（持久门禁已verified） | 主控7集成+4单元+12 E05回归及fmt/clippy通过；审批防回退、Host完整报告闭包、所有快照读入口已验；真实生产批准/组合应用/回滚受E05证据阻塞。 |
历史有限注：本行旧E06范围不扩成真实批准/生产应用；第四轮后继有具体first与Claude二验限制，仍按各原源角色分列。
| E07 | 第一个最小可验证真实闭环 | in_progress（协议与管理消费者子范围已verified）/ blocked（真实闭环） | 主控协议21项与管理增量27项、真实HTTP/MCP/CLI、参考宿主及fmt/clippy/build通过；E05注册/票据管理已接线。replay.run已由AG-001接通并验收；AG-015/AG-017 接通 curriculum.step 与 exploration.start（幂等、读侧重验；#61 merged_sha `b220971c4e5ad44453c9d3e5787fbdfda6eb01ac`、#64 merged_sha `f8fa1a19d5e968f83a52385ecb43d07d56442610`）；第二轮 AG-027（#74，merged_sha `c72aae14c9f35c1bcc090b5a374d2696d57dacfc`）管理作业与私有输入清理闭包及提交时拒绝已撤销依赖、AG-044（#89，merged_sha `cc85b5c71cec06763909fb0efa33c4e296b6178d`）提交时上游闭包闸门。meta.start 保持 blocked 直至 E14.2c；grant/package/seed 的来源与上游闭包写入闸门已由 AG-046/#96 验收；AG-059/#101已补已解析manifest dependency_refs联合首写前闸门；真实模型、独立数据与支付授权未取得。 |
| E08 | 可恢复的撤销、保留和备份链 | in_progress（当前对象/本机恢复与第二轮撤销闭包子范围已verified） | 主控47项及clippy/fmt通过；当前可信SQLite锚由操作者指定，不声称辨别假冒旧库。第二轮（均已合并）：清理只在闭包不动点完成（AG-035 #83）、同 id 第二撤销源点名拒绝（AG-036 #82）、已撤销来源不预留不派发（AG-038 #85）、经济回放记录保留（AG-037 #86）、撤销后到达的响应与 stage fact 不留明文（AG-043 #88、AG-045 #90），以及探索、课程、管理与回放对象接入清理闭包（AG-024/025/027/032）。第三轮 AG-051/#95 已补经济记录恢复保护；AG-046/#96 已补 grant/package/seed 来源与上游闭包首写前闸门及读侧闸。第四轮AG-053/#99保留依赖快照语义并前置递归限制；AG-059/#101补已解析manifest依赖首写前检查。未完成：typed边完整性、其他迟到写入与 stage_bundle；tombstone 键带 kind/发布期 kind 精确性与物理擦除无现行明确条款，待用户修订，不再误引 §11.1；Linux/生产演练另验。  AG068/#113已验新作业中native authority清理；旧Failed恢复AG069及typed边诊断AG075仅本地验收未合并，077在途，不消除其余缺口。 |
| E09 | 生成/探索解耦与有状态在线探索 | in_progress（程序协调与第二轮多项子范围已verified） | 主控25项及clippy/fmt通过；完整输入幂等、两节点StoreJournal链、源水位、整批资源已验。第二轮（均已合并）：技能组作业（AG-021 #69）、同题对比实践（AG-026 #73）、探索记录清理边（AG-024 #71）、run_next 可信观察门（AG-033 #80）、合法动作与前缀卫生（AG-039 #84）、登记指纹与计数器（AG-040 #87）、付费后收敛（AG-041 #91）、恢复计数派生（AG-042 #92）。第三轮 AG-048/#97 已验封闭终态类和白名单内固定码（模型回答日志与 broker 账本不在“不落原文”声明内）。未完成：可修复故障的类型化来源与 episode 绑定（AG-047）、优化历史进入请求与签名（PR-C）；真实 G2 未取得；V097（§3.1.1 消融）作用域未验。  AG064/#109补可信前缀失败谱系/同批episode上限与回放事实计数；真实修复链与KeepIncumbent可加深合同仍缺。 |
| E10 | 不可变世界池与纯查表回放 | in_progress（程序回放/池/报告已verified） | 主控57项、clippy/fmt通过；观察正文绑定实际共同输入和来源，q0/辅助来源篡改拒绝；世界/池/报告持久与实时撤销已验。replay.run管理适配经AG-001验收并合并；AG-032（#79，merged_sha `b262bbb2a0ea9ead68bc33fe1e58235e85bcffea`）：回放读路径在撤销清理的每个中间状态返回点名 Conflict。真实观测仍缺，不声称经济收益。  AG064/#109补Observed Recover计数投影与旧报告兼容边界；旧报告不可自动覆盖，一般Deepen/合法集/终态分类和世界封存仍未验。 |
| E11 | 验证回放优化的真实经济收益 | in_progress（合同/持久准备已verified） | 主控10项及clippy/fmt通过；单票配对、九类成本、实际预算绑定/最终回执不可变、并发取消/晚到账/报告CAS已验。可信在线配对回执消费者尚未实现；真实经济实验未运行，不声称节省。 |
| E12 | 学习者条件化的经验自主获取 | in_progress（离线子范围已verified） | 主控21项和参考宿主3个进程用例、clippy/fmt通过；控制注册、精确平台期、冷却/零预算终态、事实拒绝门已验。E03 回执 schema 已由 AG-016（#63）补齐；第二轮 AG-025（#72，merged_sha `15b140ddef25c8e794d586fecae56f129226a5c6`）课程信封清理分类、AG-027（#74）课程重放与脱敏读取 fail-closed。AG-050/#94 已验文本提案默认 unverified，以及旧内存课程 step 先选题再预留并保留 typed 错误。真实隔离（§12.0：E12 代码级范围依赖 E04 真实隔离验收）、应用正例及持久学习改变下轮选题仍未验，不启用G3。 |
| E13 | 长期部署适应与能力保留监测 | in_progress（程序监测与第二轮巩固子范围已verified） | 主控22项及clippy/fmt通过；两周期触发、单claim、真实根绑定前置校验、异常/撤销持久终态和漂移已验。第二轮（均已合并）：巩固撤销闭包、纯 pass/fail 配对与单候选暂存（AG-022 #70）、E03 门控可信周期触发（AG-028 #75）、巩固单独计量（AG-030 #77，V096.a）、终态类别有界（AG-031 #78）。第三轮 AG-048/#97 已将本卡白名单内巩固终态改为固定类型码，日志中的模型回答仍随来源闭包清理。生产周期驱动与管理入口未做（§6.1 无巩固操作，须先修订真源）；真实提供商、连续轮次保留/长期效果仍未取得。  AG-058/#105仅纠正文案：QualityStatus仍只Unknown，显式Admin漂移/观察不等于自动保留。 |
| E14 | 受限改进器自身的继承控制器 | in_progress（增量 1 与 E14.2a 程序子范围已verified并合并） | AG-019（#66，merged_sha `5a0ef4419c51687534baa3296dbbf7e310a5d810`）：有界 ElasticPolicy、决策携带 policy/caps 摘要、ImproverContentV2 只开放 exploration_policy、MechanismUsageRecordV1 只由真实派发派生；AG-034（#81，merged_sha `d9b3f6f5234ad14da18956a78c804e4c6818fa10`）：MetaTrial 分叉（同一 S0 的两条流只差 id 与 policy，按 billing scope 如实读出实耗）。未完成：ProgramFixture 范围的改进器登记（E14.2b）、meta.start 登记与绑定型消费者（E14.2c）；真批准在结构上不可达；E14 机制冻结合同为外部阻塞；不声称继承或元收益，不启用G4。  AG-058/#105仅校准MetaEvidence/指纹/实耗说明，无独立证明或每流强制限额。 |
| E15 | 后继质量与跨代收益实验 | planned | 真实后继实验未运行。 |
| E16 | 产品支持范围与最终交付门禁 | in_progress（E16.1–E16.5 子范围已验并合并） | E16.6 索引已按合并事实更新（#56）并由 AG-012 绑定 v4.2；真实宿主/沙箱/第三方包证据未取得；已按真源正文复核（2026-09-30，见顶部审计节）。2026-10-01：E16.5 容量门接线（AG-014 #62）、启动恢复/部署门（AG-018 #67）与恢复覆盖（AG-029 #76）已合并；索引登记第二轮 31 条验证记录（AG-049）。 |
| E16.1 | 来源导入与版本化读取器 | in_progress（程序/持久导入子范围已verified并合并） | PR #44 merged_sha `acda31895bb1cb42cf7985b907a4c600429573d0`（主控返修 F13–F16 后）；完整真实迁移链未关闭。  完整导入开发学习/持久候选/新使用/撤销仍必做；AG070材料与AG076执行内核仅本地验收，077预算接线在途，不伪造正式应用证据。 |
| E16.2 | 资产导入／分享与隐私门禁 | in_progress（staging/本地导出子范围已verified并合并） | PR #54 取代 #45，merged_sha `16c2817bc192bf71535d2c99d867873e6e85bcf8`（主控返修 F17–F20 后）；F08 只对照提案版；AG-059/#101补manifest来源首写前闸（登记于E08），storage-kind晚拒边界保留；真实第三方包/远端销毁未验。 |
| E16.3 | 内置种子与本地修改保护 | in_progress（持久种子安装/重置 staging 子范围已verified并合并） | PR #54 取代 #46，merged_sha `16c2817bc192bf71535d2c99d867873e6e85bcf8`；真实用户目录演练未验。 |
| E16.4 | 额外真实宿主与配置面漂移 | in_progress（拒绝门/记录重载子范围已verified并合并）/ blocked（真实宿主） | PR #47 重新堆叠后 merged_sha `1c824e6614667da4dc7ea74e96b46adfd1d2c089`（含主控 F21）；无真实 Claude Code 证据，`verify_host_receipt` 在本仓库无接受路径（fail-closed）。  AG066/#111本地真实子进程47项已验；不代表真实Claude或在途取消传播。 |
历史有限注：本行AG066本地stdio47不等真实Claude；后继真实宿主外部阻塞与独立宿主合同另行，不授本卡真实宿主信用。
| E16.5 | 持久恢复、容量、依赖与部署安全 | in_progress（门函数、容量门接线、启动恢复/部署门与恢复覆盖子范围已verified并合并）/ blocked（真实沙箱） | PR #48 merged_sha `4980aa498baa80fd82e83441839e606859fa4065`（F10–F12）；AG-014（#62，merged_sha `9c3ec979fd2ee3c53dfbf4ddf96967e94808b11e`）：容量门接入六个真实入口并实例级跨命名空间计数（F24）；AG-018（#67，merged_sha `5c4f81dca79373c6871449aed287f61fb1f3cbc2`）：启动恢复隔离门与部署门接入 rsia serve/mcp；AG-029（#76，merged_sha `7120842a1f586ae394b76e146c8298f11bec52bc`）：本轮新对象的恢复覆盖。AG-051/#95 以精确 schema 增补经济实验/作业/成本回执/报告恢复保护。AG-053/#99已验有界依赖快照（登记于E08），不声称总边扫描绝对有界。未完成：真实沙箱（外部）、Linux 实测、启动时驱动未完成的撤销清理与残留 dispatched 调用、出站一致性；本轮新增内部对象的容量上限须先修订 §3.3.1。 |
| E16.6 | 发行、证据台账与唯一真源交接 | in_progress（派生索引与检查器子范围已verified并合并） | #56 merged_sha `364a0722be7d5b05cbf01c453322ba35c2fdfa67`、#57 merged_sha `7b425fcf4da08b8949aaa8f3426853f8938d3f3c`；AG-012（#60，merged_sha `dc036d5f4dd5d870fb0ca98bdfe1ef6ec626140b`）绑定 v4.2 与谱系；2026-10-01 登记第二轮 31 条验证记录（AG-049），AG-060补登记第三轮四条、AG-061补登记第四轮首批三条已验子范围；索引仍声明 subset_only，全路线未完成；已按真源正文复核（2026-09-30，见顶部审计节）。  AG-063补登记#103–106四子范围及全部本批事件。  AG067补登记#108/#110/#109三个已验子范围及本批事件。  AG072新增登记#111/#113及六小时内全部事件；未合并本地head不纳入merged记录。 |
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


### last_raw_tail：历史任务摘要已归入上方44唯一PR节
原此批前表与事实时点完整保留；本批逐事件全文在本地只读源，新的共享运维/二验/失败分别见全局历史节。完整逐字段保全尚未闭，当前只准备而不删除原源。
## 2026-10-12 第七阶段 v4.3 切换与历史逐条复核（AG-124）

当前入口按§18.9切换至v4.3，谱系v4.1→v4.2→v4.3。v4.2已原字节归档至本地archive/2026-10-12-pre-v4.3/，摘要仍为70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455。Q-01–Q-26、29行改写表及实现缺陷见当前正文§19.5；裁决依据为本地v43/decision-table.md文末更正及v43/ledger.md。正文、归档及原始QA不上传。

派生索引、Python/Rust检查器覆盖全部23个落点任务及V001–V120。新增22族V仅planned，无执行或效果信用。历史版本、摘要、输入、源码/合并SHA、命令、计数和日志不改：91条保留记录中89条已按当前条款复核、2条E17/E18范围外；18条改变行为的完整原对象留存本地qa/codex-r7-ag124/withdrawn-records.json并撤出verified_subscopes，关联引用同步撤下。原始侦察与主控核正分开留证。E04仅更正formal_local_host已有外部provider transport接线事实，合格隔离runner未接线仍保留。

AG-124状态为implemented_not_verified，首次验收和固定head合并事实由本地codex-r7-acceptance.md留证，待Claude独立二次验收。不得声明二次验收、完整E/V、真实提供商/宿主/沙箱、实验效果、收益或发行。§18.9现有notify-merge外发已由用户在本会话明确批准并登记于docs/ci-egress-register.json：原工作流合并后将完整GitHub pull_request事件JSON（PR标题/正文/作者/仓库/提交元数据）POST至既有Cursor webhook，使用GROKBOT_WEBHOOK_KEY密钥名。接收方、密钥与数据范围不改，未读取密钥值；CI逐步强制核对检查器仍待Q24后续任务。登记schema表示待二次验收裁决。

### 范围解释与待二次验收裁决

- AG097/105被撤的断言是未脱敏缓存拒绝前补execution_close；§11.3.1/V102.b要求该入口零写入。§11.6/V111.b对严格同bound身份已脱敏请求的恰好一次关闭仍保留，不扩成一般豁免。
- AG065 exact clone按§5.5.1保留名称信用；短source_digest夹具不授Q25来源绑定信用。AG039不从Recover辅助用例扩大Deepen范围，AG070不从native格式setup扩大有界物料范围，AG092不从P/B夹具扩成摘要合法性。解释待二验，新义务另卡。
- 原不可变ctrl_verify.sh固定v4.2根路径；本地副本只适配PLAN一行，八门及原始子日志保留，待二验。不改原脚本、不冒称它读取v4.3通过。

### 109条逐条判定（源码行号为基线main 6a5b5ebf，真源行号为当前v4.3）

| E | 完整 record id | 判定 | Q／现读真源依据 | 实际测试／源文件行号与理由 |
|---|---|---|---|---|
| E00 | E00.controller_acceptance | 行为未变 | Q15/Q18/Q24；E00 L1523–1525；§18.9 L2832 | `crates/evo-engine/src/evaluator.rs:182–273`：定向拒绝不完整行、票据失配、无效统计；未声明新 CI、许可或工具链义务。 |
| E00 | E00.ag099_step_future_default_test_stack | 行为未变 | Q26；E00 L1523；§18.9 L2832 | `crates/evo-engine/tests/step_terminal_classes_v42.rs:1476–1960,2407–2547`：装箱降低 step future 栈占用，终态与决策语义保持；新 V 注册不使旧默认栈执行证据失效。 |
| E01 | E01.controller_acceptance | 行为未变 | Q04/Q16/Q22；E01 L1541–1543；§19.5 L2970 | `crates/evo-core/tests/evaluation_v41.rs:122–264,388–531`：连续前缀、UCB、研究族额度、锚点和零预算静态合同；统计公式未改，新计划绑定／任务登记另验。 |
| E01 | E01.ag057_explicit_sample_registration | 行为未变 | Q16/Q22；E01 L1541–1543 | `crates/evo-core/tests/evaluation_contract_v42.rs:60–123`：显式 n 保留、越界拒绝、旧 v1 字节不变；不把显式 60 当正式功效达标。 |
| E02 | E02.controller_acceptance | 行为未变 | Q25；E02 L1557–1559 | `crates/evo-core/tests/skill_edit.rs:92–154,442–571`：UTF-8 前像、锚点、整包原子性和确定性；新摘要字段登记不改变这些编辑行为。 |
| E02 | E02.ag055_v1_wire_goldens | 行为未变 | Q08/Q26；§5.5.1 L595；E02 L1557 | `crates/evo-core/tests/v1_golden.rs:39–56,83–142,145–243`：旧请求、Strategy、Evaluation 的固定字节／缺省和拒绝未知字段；v1 golden 保留。 |
| E02 | E02.ag062_required_improver_baselines | 行为被改变 | Q25；§5.1.2 L517；E02 L1559 | `crates/evo-core/tests/improver_required_baselines_v42.rs:415–452`：明确接受 `"P"`、`"B"`、空白、任意中文标签及 129 字符标签进入 `*_digest`／`source_digest`；新摘要通则禁止该边界。 |
| E02 | E02.ag065_host_surface_name_coverage | 行为未变 | Q08；§5.5.1 L595；Q25 新义务另验 | `crates/evo-core/tests/host_surface_name_coverage_v42.rs:39–99`，`crates/evo-engine/tests/host_surface_name_coverage_v42.rs:299–314`：名称遗漏拒绝，重复项是原项 exact clone；真源明确保留完全相同重复项。旧 source_digest 夹具争议另列。 |
| E02 | E02.ag092_local_scoped_acceptance | 行为未变 | Q03/Q25；§11.3.1 L1397–1403；§18.9 L2832 | `crates/evo-core/tests/baseline_dependency_revocation_v42.rs:196–315,428–512`：已知 P/B 直接依赖撤销不可被 Set/Reset 隐藏；未声明可信闭包或摘要合法性。 |
| E02 | E02.ag093_local_scoped_acceptance | 行为未变 | Q25；E02 L1557–1559；§18.9 L2832 | `crates/evo-engine/tests/compiler_input_binding_v42.rs:41–155,158–335`：原输入解析、权威见证、typed 版本与绑定；显式 Admin fixture 无普通 P、provider 或效果信用。 |
| E03 | E03.controller_acceptance | 行为未变 | Q07/Q19；E03 L1573–1575 | `crates/evo-engine/tests/optimization.rs:496–708,710–835`：程序 fixture 单步消费者、journal 与开发选择；不声明 Tool-only 挂载或生产失败铸造。 |
| E03 | E03.ag016_development_receipts | 行为未变 | Q07；E03 L1573；§18.9 L2832 | `crates/evo-engine/tests/development_receipts.rs:451–828,1283–1440`：分离执行／评分身份、服务器重评分、受信观察门与撤销；新增修复模板／重置收据另验。 |
| E03 | E03.ag079_local_scoped_acceptance | 行为未变 | Q07/Q25；E03 L1573–1575 | `crates/evo-core/tests/diagnosis_hypothesis_structure_v42.rs:368–447,462–490`：复制假设前结构拒绝、七类原因、重复引用；合法 fixture 摘要为 hash，不授予修复资格。 |
| E04 | E04.controller_acceptance | 行为未变 | Q04/Q09；§6.1.1 L715–722；E04 L1589–1591 | `crates/evo-storage/tests/budget.rs:74–410,456–517`：持久根预留／dispatch／结算、取消已派发不退款；不是内存 `RootBudget::cancel` 覆盖 Dispatched 的缺陷。 |
| E04 | E04.ag054_typed_predispatch | 行为未变 | Q09/Q12；§6.1.1 L720；§11.6 L1448–1450 | `crates/evo-engine/tests/broker.rs:247–378,982–1260`：已知首派前拒绝、仅 Reserved 释放、Dispatched/Uncertain 保留；脱敏严格形状新增守卫另验。 |
| E04 | E04.ag082_local_scoped_acceptance | 行为未变 | Q04；E04 L1589–1591；§18.9 L2832 | `crates/evo-storage/tests/budget_sql_fault_rollback_v42.rs:584–592`：单／多 namespace 根授权事件失败回滚与同请求恢复；新阶段计划绑定不改变事务回滚。 |
| E04 | E04.ag077_local_scoped_acceptance | 行为未变 | Q03/Q12；E04 L1591；E08 L1653 | `crates/evo-storage/tests/e16_budget_binding_v42.rs:140–243`：typed E16 来源快照及引用丢失后的费用／清理；记录明确 library-only，不等于生产三闸闭包已验。 |
| E04 | E04.ag097_broker_close_before_refuse | 行为被改变 | Q03；§11.3.1 L1397–1403；V102.b L2235 | `crates/evo-engine/tests/broker_cache_revoke_v42.rs:870–885,1193–1211`：未脱敏已结算缓存，begin_revoke 后被拒仍先写 execution_close；违背被拒缓存入口全表零写入。严格同身份已脱敏关闭特例保留。 |
| E04 | E04.ag105_broker_bound_recovery_close_v42 | 行为被改变 | Q03；§11.3.1 L1401；V102.b L2235；§11.6 L1450 | `crates/evo-engine/tests/broker_bound_recovery_close_v42.rs:891–934`：未脱敏来源撤销的 bound replay，拒绝前补关闭；整条撤回，`945–1001` 的已脱敏同身份收口义务不取消。 |
| E05 | E05.controller_acceptance | 行为未变 | Q05/Q09/Q16；E05 L1605–1609 | `crates/evo-engine/tests/streaming_evaluator.rs:748–1067,1242–1459`：真实 receipt、乱序连续前缀、停机阻断、费用／曝光保留；新增历史判定版本表另验。 |
| E05 | E05.ag052_versioned_batch_verdicts | 行为未变 | Q05；§5.1.1 L505–513；E05 L1605 | `crates/evo-engine/tests/streaming_verdicts_v42.rs:815–958,1085–1164`：显式控制版本、v1 固定字节与新统计后处理；没有把所有旧回放 v2 可消费性并入该范围。 |
| E05 | E05.ag083_partition_admission_rejection_atomicity | 行为未变 | Q16；E05 L1605–1607；§18.9 L2832 | `crates/evo-core/tests/partition_admission_atomic_v42.rs:165–398,771–851`：拒绝无部分成员／家族写入，曝光历史保留；新受控任务登记另验。 |
| E05 | E05.ag080_local_scoped_acceptance | 行为未变 | Q05；E05 L1605 | `crates/evo-engine/tests/evaluator_plan_binding_v42.rs:136–317,452–650`：QueryBook／Evaluator 冻结计划绑定、失配前拒绝，不增加正式收益信用。 |
| E05 | E05.ag087_local_scoped_acceptance | 行为未变 | Q03/Q05；§11.3.1 L1397–1403；E05 L1605 | `crates/evo-engine/tests/formal_candidate_dependency_binding_v42.rs:923–1256,1416–1494`：固定候选与完整来源绑定，拒绝不写票据／费用／曝光；新版本表另验。 |
| E05 | E05.ag088_local_scoped_acceptance | 行为未变 | Q05；E05 L1605；§19.5 L2970 | `crates/evo-engine/tests/complete_batch_statistics_binding_v42.rs:1062–1081`：报告均值篡改拒绝，消费真实统计；公式与判定顺序保留。 |
| E05 | E05.ag089_local_scoped_acceptance | 行为未变 | Q05；§5.1.1 L505–513；E05 L1605 | `crates/evo-engine/tests/early_stop_certificate_binding_v42.rs:1271–1465,1531–1655`：绑定实际首个证书、前缀与迟到费用；不声称持久版本谱系新增义务已验。 |
| E05 | E05.ag090_local_scoped_acceptance | 行为未变 | Q05/Q09；E05 L1605；§6.6 L880–884 | `crates/evo-engine/tests/invalid_formal_reason_binding_v42.rs:1225–1301`：invalid 理由绑定实际 Some-ticket 付费事实，替换／重排拒绝。 |
| E05 | E05.ag091_local_scoped_acceptance | 行为未变 | Q09；§6.6 L880–884；E05 L1605 | `crates/evo-engine/tests/late_grader_stop_preservation_v42.rs:1115–1197,1411–1561`：迟到 grader 错误不撤销已确认早停，费用和未决状态保存。 |
| E05 | E05.ag095_canonical_grader_receipt_digest | 行为未变 | Q25；§5.1.2 L517；E05 L1605 | `crates/evo-engine/tests/formal_external_host_authority_v42.rs:1451–1640`：排序值的可重算 receipt 摘要、capture 绑定与伪造拒绝；符合新摘要方向。 |
| E05 | E05.ag101_streaming_verdicts_v42 | 行为未变 | Q05；E05 L1605；§19.5 L2970 | `crates/evo-engine/tests/streaming_verdicts_v42.rs:959–1048`：仅 UCB<0 确认退化，成本越限／重启不覆盖该结论；统计规则未改。 |
| E05 | E05.ag109_external_host_authority_bound_ticket | 行为未变 | Q03/Q05；E05 L1605；§11.3.1 L1397–1403 | `crates/evo-engine/tests/formal_external_host_authority_v42.rs:1829–2121`：已 staging 候选票据发行、权威边精确集、额外不一致边拒绝；不新增正式发布效果。 |
| E05 | E05.ag100_formal_candidate_dependency_binding_v42 | 行为未变 | Q05；E05 L1605 | `crates/evo-engine/tests/formal_candidate_dependency_binding_v42.rs:1746–1856`：任何控制 schema 缺候选时首写前拒绝，不退回旧无绑定路径。 |
| E05 | E05.ag118_late_receipts_after_final_report_v42 | 行为未变 | Q09；§6.6 L880–884；E05 L1605 | `crates/evo-engine/tests/late_grader_after_final_report_v42.rs:1685–1773`：最终报告后迟到 receipt／错误只留实际审计，原报告或证书保留；符合不可覆盖副作用事实方向。 |
| E06 | E06.controller_acceptance | 行为被改变 | Q19；§5.5.2 L618–620；E06 L1623 | `crates/evo-engine/tests/release_store.rs:74–88,300–318,384–402,509–519`：v1 `host.model=supported` 搭配 ToolOnly／Attached 快照接受；新 manifest 能力上限改变这些正向行为。 |
| E06 | E06.ag084_local_scoped_acceptance | 行为未变 | Q02/Q19；E06 L1617–1623 | `crates/evo-engine/tests/candidate_review.rs:707–966,1084–1311`：只读候选／stage-fact 审阅，不给予历史、正式评测或批准信用；新发布登记／attached 门另验。 |
| E06 | E06.ag116_edit_compiled_produce_review_v42 | 行为未变 | Q25；§5.1.2 L517；E06 L1617 | `crates/evo-engine/tests/edit_compiled_produce_review_v42.rs:715–882`：真实 producer 规则计算摘要，一字节变更拒绝；StepPrepared 物料绑定未改。 |
| E07 | E07.controller_acceptance | 行为未变 | Q09/Q14/Q16；§6.1.1 L715–720；E07 L1637–1639 | `crates/evo-http/tests/service.rs:178–426`：认证／身份／幂等与 disabled-model 范围；`394–423` 是 blocked 后 cancel 仍 blocked，无已提交业务结果被覆盖。 |
| E07 | E07.ag015_curriculum_step_consumer | 行为未变 | Q02/Q09/Q10；§6.1.1 L716；§18.9 L2832 | `crates/evo-engine/tests/dispatch_management.rs:1235–1757`：Admin curriculum.step、原子 probe／状态／边／receipt 与终态重核；`1728–1757` 取消在 claim 前，无业务副作用。 |
| E07 | E07.ag017_exploration_start_consumer | 行为未变 | Q02/Q09；§6.1.1 L716；§18.9 L2832 | `crates/evo-engine/tests/dispatch_management.rs:2290–3071`：Admin world 注册、幂等纯决策与来源边；`3035–3071` queued-before-claim cancel，不固定提交后覆盖。 |
| E07 | E07.ag027_management_cleanup_closure | 行为未变 | Q03/Q09；§11.3.1 L1397–1403；§6.1.1 L720 | `crates/evo-engine/tests/management_cleanup_v42.rs:1828–2031,3177–3445`：job 保留、private input 脱敏、已撤销依赖提交前拒绝；清理不是可取消业务作业。 |
| E07 | E07.ag044_submit_upstream_gate | 行为未变 | Q03；§11.3.1 L1397–1403 | `crates/evo-engine/tests/submit_upstream_gate_v42.rs:1811–1880,2152–2342,2407–2538`：完整有界上游闭包、kind 精确、拒绝零写入；同 key 仍取历史 job。 |
| E07 | E07.ag108_candidate_review | 行为未变 | Q25；§5.1.2 L517；E06 L1617 | `crates/evo-engine/tests/candidate_review.rs:1540–1793`：producer 原规则摘要才给 exact diff，bare-rule／混合 compiler-key 形状拒绝。 |
| E08 | E08.controller_acceptance | 行为未变 | Q03/Q11/Q13；E08 L1653–1655 | `crates/evo-storage/tests/lifecycle.rs:246–460,1027–1238,1408–1466`：即时封锁、分页清理、不可逆费用保留、独立锚恢复；新物理残留／权限／锁义务另验。 |
| E08 | E08.ag036_revocation_source_collision | 行为未变 | Q03；§11.3.1 L1395–1400 | `crates/evo-storage/tests/revocation_source_collision.rs:262–307,312–365`：已有另一 kind tombstone 的第二次撤销 Conflict／NotFound，旧水位／对象／作业／全表不变；不把存量同 id 冲突隔离新增义务标已验。 |
| E08 | E08.ag035_cleanup_fixpoint | 行为未变 | Q03；E08 L1653–1655 | `crates/evo-storage/tests/cleanup_fixpoint.rs:235–342,586–775`：Complete 前重扫晚依赖和预算请求，未知对象失败、重启续清；新增入口登记另验。 |
| E08 | E08.ag038_budget_revoke_gate | 行为未变 | Q03/Q09；§11.3.1 L1397–1403；§6.1.1 L720 | `crates/evo-storage/tests/budget_revoke_gate.rs:345–500,585–726,779–913`：首写前预留守卫、kind 精确、已派发实际费用继续结算；不授予未脱敏缓存拒绝前关闭信用。 |
| E08 | E08.ag037_economic_replay_cleanup | 行为被改变 | Q05/Q06；§7.4.3 L1088；§5.1.1 L512 | `crates/evo-engine/tests/replay_economic_cleanup_v42.rs:465–482,1244–1261`：v2 report 进入经济 experiment／真实 start 的成功控制；现在 legacy 选择必须被登记／启动拒绝。 |
| E08 | E08.ag043_late_response_revoked | 行为未变 | Q03/Q12；§11.6 L1441–1452；E08 L1653 | `crates/evo-storage/tests/late_settlement_revoked.rs:1–18` 及该目标结算测试：已派发迟到费用保留、response 封锁、transport 脱敏；不把新派发拒绝等同已发生费用删除。 |
| E08 | E08.ag045_late_stage_fact_redaction | 行为未变 | Q03/Q12；§11.5；E08 L1653–1655 | `crates/evo-engine/tests/late_stage_fact_redaction_v42.rs:1385–1515,1518–1563,2149–2156`：已派发观察事实脱敏、kind 精确、真实费用留存；budget 工件新五字段守卫另验。 |
| E08 | E08.ag046_write_side_revocation | 行为被改变 | Q03；§11.3.1 L1399–1401；V102.b L2235 | `crates/evo-engine/tests/write_side_revocation_gate_v42.rs:2768–2860`：known_boundary 固定另一 kind tombstone 导致首写后 Forbidden，且 Prepared 剩一条；新规则要求精确 kind／拒绝零写入。 |
| E08 | E08.ag053_bounded_dependency_snapshots | 行为未变 | Q03/Q01；§11.3.1 L1398；E08 L1653 | `crates/evo-storage/tests/local_export.rs:135–199,304–611`：依赖快照与 blob 前检有界、namespace／顺序／指纹保持；发布锁／权限新增义务另验。 |
| E08 | E08.ag059_manifest_prewrite_gate | 行为被改变 | Q03；§11.3.1 L1399–1401 | `crates/evo-engine/tests/write_side_revocation_gate_v42.rs:2179–2323,2768–2808`：正常联合门零写入之外，同目标仍把另一 kind 的晚拒及 Prepared／audit／edge 残留固定为 known boundary；整条不能拆取继续计入。 |
| E08 | E08.ag068_native_authority_cleanup | 行为未变 | Q03/Q20；E08 L1653；§18.9 L2832 | `crates/evo-engine/tests/import_source_successor_cleanup_v42.rs:374–392,395–505,796–864`：有效内部原生权威的 typed 后继逻辑清理；不是把内部权威 schema 当 Trace 交换格式。 |
| E08 | E08.ag069_local_scoped_acceptance | 行为未变 | Q02/Q03；§6.1.1 L720；E08 L1653 | `crates/evo-storage/tests/failed_cleanup_resume_v42.rs:248–463,888–1183`：Admin 修复后整前沿重试、Worker 续清与旧失败历史保留；生产 cleanup.continue 新入口另验。 |
| E08 | E08.ag073_local_scoped_acceptance | 行为未变 | Q03；§11.3.1 L1397–1403 | `crates/evo-engine/tests/broker_cache_revoke_v42.rs:376–610,676–706`：已完成缓存对直接／上游来源撤销拒绝、不重付；不把后来 AG097 增加的补 close 计入本记录。 |
| E08 | E08.ag075_local_scoped_acceptance | 行为未变 | Q03；E08 L1653；§18.9 L2832 | `crates/evo-storage/tests/dependency_read_v42.rs:35–153,155–243`：typed 来源／结果边、分页和 namespace 精确只读绑定；明确 library-only。 |
| E08 | E08.ag096_unreadable_bound_request_isolated | 行为未变 | Q12/Q25；§11.6 L1452；E08 L1653 | `crates/evo-storage/tests/cleanup_unreadable_bound_request_v42.rs:698–842,972–1054`：破损 JSON／版本／closure 单行隔离；未知非来源字段不阻碍破坏性清理，不授予 use-path 合法性。 |
| E08 | E08.ag107_budget_ref_missing_fail_closed_v42 | 行为未变 | Q03/Q12；§11.3.1 L1398–1400；E08 L1653 | `crates/evo-storage/tests/budget_ref_missing_fail_closed_v42.rs:518–830`：引用丢失仍由存储来源闭包阻断撤销／不可核验派发，迟到费用结算并封锁。 |
| E08 | E08.ag106_budget_startup_recovery_v42 | 行为未变 | Q09/Q13；§6.1.1 L722；E08 L1653–1655 | `crates/evo-storage/tests/budget_startup_recovery_v42.rs:565–822,899–958`：启动补已结算失落 close、第二遍不写、不可收口仍留持有；新正式宿主持锁启动接线另验。 |
| E08 | E08.ag104_restore_prefix_migration_upgrade_v42 | 行为未变 | Q15；E08 L1653；§16.7 L2686 | `crates/evo-storage/tests/restore_prefix_upgrade_v42.rs:835–924,969–1046`：已登记 checksum 的严格旧迁移前缀升级，gap／未知／失败迁移隔离；不重新生成锁或放宽旧备份。 |
| E08 | E08.ag117_formal_call_revoke_cleanup_v42 | 行为未变 | Q03/Q12；E08 L1653；§11.6 L1448 | `crates/evo-storage/tests/formal_call_revoke_cleanup_v42.rs:619–856,1204–1268`：清理使用已登记来源引用、保留账单／关闭事实、不相关行不变；新内容 use-path 守卫另验。 |
| E09 | E09.controller_acceptance | 行为未变 | Q02/Q07；E09 L1669–1671 | `crates/evo-engine/tests/exploration_v41.rs:495–552,1164–1268`：确定性程序 fixture 协调、真实调用既有单步消费者、历史与中间节点拒发布；未声明新增修复资格。 |
| E09 | E09.ag021_skill_group_jobs | 行为未变 | Q01/Q02；§3.3.2 L331；E09 L1669 | `crates/evo-engine/tests/skill_groups_v42.rs:863–1028,1271–1301`：最多两组、先验全请求校验、组间失败隔离、共根预算；新增部署并发对象容量另验。 |
| E09 | E09.ag024_exploration_cleanup_edges | 行为未变 | Q03；E09 L1665；E08 L1653 | `crates/evo-engine/tests/exploration_cleanup_v42.rs:1187–1200,1514–1725,1769–1937`：node／dispatch／history 首写同事务 world 边与脱敏读拒绝；未引入新可修复来源信用。 |
| E09 | E09.ag026_practice_attempt_sets | 行为未变 | Q01/Q02；§3.3.2 L331；E09 L1669 | `crates/evo-engine/tests/practice_v42.rs:957–1212,2425–2764`：K1／授权单集 K3、不增独立 n／周期、并发一集与缓存非新实践；新增实例总集容量另验。 |
| E09 | E09.ag033_run_next_trusted_gate | 行为未变 | Q07；§7.2.2 L1012–1017；E09 L1671 | `crates/evo-engine/tests/exploration_trust_v42.rs:1497–1803,2194–2315`：fixture／伪证非有效观测，真实 receipts 才可加深／提候选，无改动 hard_failure／not_observed；符合新非观测终态。 |
| E09 | E09.ag039_legal_actions | 行为未变 | Q07；§7.2.2 L1012–1022 | `crates/evo-engine/tests/exploration_legal_actions_v42.rs:1656–1895`：声明仅 Trusted Deepen、已展开父不重复加深、child best 继承；`1981–2042` 的 Recover 辅助回归不扩大该 scope。 |
| E09 | E09.ag040_exploration_registration | 行为未变 | Q02/Q07；E09 L1665–1671 | `crates/evo-engine/tests/exploration_start_after_dispatch_v42.rs:1146–1331,1664–1792,2410–2474`：不可变注册指纹、真实 dispatch 反推计数与成本、重注册／status 原事实绑定。 |
| E09 | E09.ag041_postpaid_convergence | 行为未变 | Q07/Q09；E09 L1669；§6.1.1 L720 | `crates/evo-engine/tests/exploration_postpaid_v42.rs:1311–1583,1735–1964`：已付费后的前缀／claim 漂移或目标丢失收为唯一 Uncertain，支付事实不丢；新增首派前修复授权另验。 |
| E09 | E09.ag042_recovery_counting | 行为被改变 | Q07；§7.2.3 L1026–1030 | `crates/evo-engine/tests/exploration_recovery_counting_v42.rs:1503–1542,2759–2765`：原 scope 包括 Recover 资格；直接改 storage status、任意 episode／模板 hash／reset bool 即开放 Implementation 等 Recover，缺可信定位、登记模板和重置收据。 |
| E09 | E09.ag048_terminal_classes | 行为未变 | Q07；§7.2.2 L1010–1017 | `crates/evo-engine/tests/step_terminal_classes_v42.rs:2337–2404,2407–2547`：显式 no_change／kept_incumbent class，无有效候选、不可 Deepen／提最终候选；类别与非观测行为一致。 |
| E09 | E09.ag074_local_scoped_acceptance | 行为被改变 | Q05/Q06；§7.4.3 L1088 | `crates/evo-engine/tests/replay_legal_prefix_v42.rs:578–637,639–698`：旧 v2 未受影响报告仍 verified／可同 ID 幂等重算；新规则所有 v2 一律 legacy，不能恢复消费性。 |
| E09 | E09.ag103_fairness_waits_legal_set_v42 | 行为未变 | Q06/Q07；E09 L1669–1671 | `crates/evo-core/tests/fairness_waits_legal_set_v42.rs:419–471,473–514,579–636`：公平等待只对完成轮中的合法跳过机会，纯过滤和决定等价；不是持久 v2 report 消费信用。 |
| E10 | E10.controller_acceptance | 行为被改变 | Q05/Q06；§7.4.2 L1075–1084；§7.4.3 L1088 | `crates/evo-engine/tests/replay_v41.rs:356–424,678–789`：固定旧 PolicyStop／WorldExhausted 分流并正向 verified v2 持久报告；新终止次序及全部 v2 legacy 改变范围。 |
| E10 | E10.replay_management_controller_acceptance | 行为被改变 | Q05/Q06；§7.4.3 L1088 | `crates/evo-engine/tests/dispatch_management.rs:719–803,1011–1073`：replay.run 生成／消费现 v2 report，status／崩溃恢复按当前报告复验；须随 v3／legacy 新合同重验。 |
| E10 | E10.ag032_replay_redacted_reads | 行为未变 | Q03/Q05；§5.1.1 L508–509；§11.3.1 L1399–1400 | `crates/evo-engine/tests/replay_redacted_reads_v42.rs:909–1231,1314–1514`：声明仅各清理阶段 redacted 命名 Conflict 和精确 kind／不可读拒绝；legacy 仍可读／审计／清理，不给当前 verified 信用。 |
| E10 | E10.ag064_replay_episode_recovery | 行为被改变 | Q05/Q06/Q07；§7.4.3 L1088 | `crates/evo-engine/tests/replay_recovery_contract_v42.rs:711–757,759–782`：实际旧 v2 单次恢复报告仍 verified／可同 ID 重跑；全部 v2 legacy 合同改变其正向解释。 |
| E10 | E10.ag103_replay_fairness_waits_legal_set_v42 | 行为被改变 | Q05/Q06；§7.4.2 L1075–1084；§7.4.3 L1088 | `crates/evo-engine/tests/replay_fairness_waits_v42.rs:357–392,636–721,723–759`：固定旧深度帽终态及旧 v2 verified／幂等；纯公平过滤保留不代表整条持久回放范围未变。 |
| E11 | E11.controller_acceptance | 行为未变 | Q05/Q21/Q22；E11 L1701–1703 | `crates/evo-core/tests/replay_economics.rs:188–264,266–303`：静态完整成本、unknown 非零、盈亏平衡与不发布边界；不包含实际 legacy selection 登记／启动正向消费。 |
| E12 | E12.controller_acceptance | 行为未变 | Q10；§8.1.1 L1197；E12 L1717–1719 | `crates/evo-engine/tests/curriculum_v41.rs:150–209,211–313,315–359`：零预算、无 sandbox、untyped／missing fact 拒绝；没有把回归写成 failure_gap 的成功断言。 |
| E12 | E12.ag025_curriculum_cleanup | 行为未变 | Q03/Q10；E08 L1653；E12 L1717 | `crates/evo-engine/tests/curriculum_cleanup_v42.rs:879–989,991–1250,1628–1673`：既有九类记录保留／脱敏分类、来源闭包和续清；v3 失败状态／三臂新对象另验。 |
| E12 | E12.ag050_curriculum_priority | 行为未变 | Q04/Q10；E12 L1717–1719 | `crates/evo-engine/tests/curriculum_priority_fixes_v42.rs:127–175,199–213`：旧内存 step 先选题再预留、typed 错误与文本不自签 oracle；不声明生产 LearnerState v3 或可学习分支已通。 |
| E13 | E13.controller_acceptance | 行为被改变 | Q19；§5.5.2 L620；E06 L1623 | `crates/evo-engine/tests/monitoring.rs:58–71,149–167,860–878`：v1 host.model supported 即接受 Attached run；新 attached 必须 v2 skill.content／正确 consumer／行为证明。 |
| E13 | E13.ag022_consolidation_closure | 行为未变 | Q02/Q10；E13 L1729–1735 | `crates/evo-engine/tests/monitoring_consolidation_v42.rs:1430–1531,1723–1851,2386–2512,2766–2991`：pass/fail 配对、proposal／唯一 staging、来源闭包；新生产 trigger/run 和复验选题另验。 |
| E13 | E13.ag028_consolidation_trigger | 行为被改变 | Q02；§6.1.2 L777 | `crates/evo-engine/tests/monitoring_trusted_cycle_v42.rs:1866–1884`：四周期后仅 claim generation2、claim_ids=[2]，明确把 generation1 永久静默跳过固定为成功；新要求 no_contrast／generation_expired 显式终结。 |
| E13 | E13.ag030_consolidation_budget_stage | 行为未变 | Q04/Q09；§3.5.1 L424；§6.1.1 L720 | `crates/evo-engine/tests/consolidation_budget_stage_v42.rs:377–430,1568–1766,1869–2004`：Consolidate⇔Consolidation 一致、错误阶段拒绝、实际 timeout／cancel 费用保留；新计划绑定另验。 |
| E13 | E13.ag031_bounded_terminal_category | 行为未变 | Q02/Q09；E13 L1733–1735 | `crates/evo-engine/tests/consolidation_terminal_category_v42.rs:133–257,1164–1336`：128-byte UTF-8 安全类别、长拒绝／uncertain 收口；不把业务取消覆盖已提交结果算信用。 |
| E13 | E13.ag081_local_scoped_acceptance | 行为未变 | Q10/Q25；E13 L1733；§5.1.2 L517 | `crates/evo-engine/tests/monitoring_evidence_binding_v42.rs:1475–1583,1793–1968,2032–2095`：原 report、pair、执行输入和 receipt 的实际绑定／篡改拒绝；新失败当前性另验。 |
| E14 | E14.ag019_exploration_policy_inheritance | 行为未变 | Q23/Q22；§5.7.1 L661；E14 L1749–1751 | `crates/evo-engine/tests/meta_inheritance_v42.rs:749–870,1027–1307,1754–2060`：受限 policy、实际 dispatch 的 policy/caps 摘要与机制使用，不靠 used flag；新机制资产发布另验。 |
| E14 | E14.ag034_metatrial_fork | 行为未变 | Q01/Q22/Q23；E14 L1749–1751 | `crates/evo-engine/tests/meta_trial_v42.rs:1005–1157,1407–1646,1832–1936,2731–2858`：同 S0 双流，仅 id/policy 不同、费用／事实隔离、撤销／恢复；未声明统计收益或冻结合同通过。 |
| E14 | E14.ag058_current_description_boundaries | 行为未变 | Q02/Q23；E14 L1749–1751；§18.9 L2832 | `crates/evo-engine/tests/meta_trial_v42.rs:2647–2728,3223–3261`：描述与消费者事实校正、可变消耗不改不可变注册身份；未对未接入口授予信用。 |
| E16.1 | E16.1.durable_import_controller_acceptance | 行为被改变 | Q20；§11.2.1 L1381–1385；E16.1 L1797 | `crates/evo-engine/tests/import_v41.rs:209–214,362–374`：声明三个 pinned readers，正向接受缺 exporter／event seq／coverage 的 native Trace，后一例连 run_id 也缺；新交换合同收紧。 |
| E16.1 | E16.1.ag076_local_scoped_acceptance | 行为未变 | Q20/Q25；E16.1 L1793–1797；§18.9 L2832 | `crates/evo-core/tests/import_fixture_v42.rs:283–327,607–737,1024–1095`：有界 material 的实际程序 fixture 操作／匹配／闭包／编码；不声明源格式读取器或可信执行。 |
| E16.1 | E16.1.ag070_local_scoped_acceptance | 行为未变 | Q20新增格式义务另验；§18.9 L2832；待二次验收裁决 | `crates/evo-engine/tests/import_development_material_v42.rs:326–380`：原scope仅bounded local development-material reader；`29–35`的native格式注册在setup，不把setup或整目标用例数扩成源格式支持信用。有界物料、摘要/截断和只读行为保留，新入口格式另验。 |
| E16.2 | E16.2.durable_staging_publish_controller_acceptance | 行为未变 | Q03/Q15/Q17/Q20；E16.2 L1803–1807；§18.9 L2832 | `crates/evo-engine/tests/packages_v41.rs:873–982,1155–1309,1768–1894`：目录包 durable staging／publish、真实摘要、来源撤销与本地交付；不授予 ZIP 解析、native Trace 导出或新内置 catalog 信用。 |
| E16.2 | E16.2.ag071_known_marker_utf8_blocking | 行为未变 | Q03/Q20；E16.2 L1803；§18.9 L2832 | `crates/evo-engine/tests/privacy_utf8_v42.rs:156–204,517–655`：原 byte cap 内敏感 marker UTF-8 安全阻断、首写前拒绝；没有改变隐私政策。 |
| E16.2 | E16.2.ag085_utf8_privacy_boundary_regressions | 行为未变 | Q20；E16.2 L1803；§18.9 L2832 | `crates/evo-engine/tests/privacy_utf8_boundary_v42.rs:120–151,321–343,445–483`：12 个真实 UTF-8 边界、原内容／摘要与拒绝回归；原 scanner 未改。 |
| E16.3 | E16.3.seed_install_reset_controller_acceptance | 行为未变 | Q17；§11.1.1 L1363；E16.3 L1813–1817 | `crates/evo-engine/tests/seeds_v41.rs:33–160,434–502`：B/L/U 均为 hash、双方完整 asset/kind/publisher 身份；安装／reset staging 不 auto-Active。短 B／缺身份库单测不在本记录。 |
| E16.4 | E16.4.real_store_rejection_controller_acceptance | 行为被改变 | Q19；§5.5.2 L618–620；E16.4 L1827 | `crates/evo-engine/tests/hosts_v41.rs:32–67,183–218` 与 `crates/evo-engine/src/hosts.rs:45–71`：结构 fixture 将 RSIA 自身四工具列 supported，并建立 v1 supported ToolOnly run；新零 supported／RSIA 工具非宿主面规则改变该宽记录。 |
| E16.4 | E16.4.ag066_local_stdio_harness | 行为未变 | Q09/Q19；§6.1.1 L713；E16.4 L1825–1827 | `crates/evo-mcp/tests/service.rs:76–174,177–218`：本地 subprocess SDK handshake、四工具 schema、persist/reconnect、重复键首派前拒绝；未给真实 Claude、attached、取消能力信用。 |
| E16.5 | E16.5.gate_functions_controller_acceptance | 行为未变 | Q01/Q11/Q13；E16.5 L1833–1837 | `crates/evo-engine/tests/capacity_v41.rs:137–284,286–486,544–794`：既有容量／恢复／部署纯函数边界，拒绝 sandbox bool 冒充证据；新对象、权限、可信时钟及真实启动义务另验。 |
| E16.5 | E16.5.ag014_capacity_gates_wired | 行为未变 | Q01；§3.3.2 L331；E16.5 L1835–1837 | `crates/evo-engine/tests/capacity_v41.rs:796–867`，`crates/evo-engine/tests/dispatch_management.rs:1914–1965`，`crates/evo-engine/tests/packages_v41.rs:2248–2309`：原对象限值与跨 namespace 容量一致；不授予新增内部对象容量。 |
| E16.5 | E16.5.ag018_startup_recovery_gate | 行为未变 | Q11/Q13；§7.3.1 L1040；E16.5 L1833–1837 | `crates/evo-engine/tests/startup_gate_v42.rs:296–624,625–808,1354–1437`：已有 serve gate 只读复核、独立锚、锁占用拒绝、无覆盖准入回执；正式宿主新接线／文件模式另验。 |
| E16.5 | E16.5.ag029_restore_coverage | 行为未变 | Q03/Q05；E16.5 L1833–1837 | `crates/evo-engine/tests/restore_coverage_v42.rs:1024–1207,1385–1430,1510–1570`：现有 action schema Rust/Python 选择集合相等、锚前进隔离；新对象按后续实际引入增补，不改旧事实。 |
| E16.5 | E16.5.ag051_economic_restore | 行为未变 | Q05/Q06；§5.1.1 L508–512；E16.5 L1833 | `crates/evo-engine/tests/restore_economic_v42.rs:1248–1346,1487–1622`：只声明经济记录保护／恢复精确事实；旧派生记录原样保留，不声称 legacy 可用于新选择或实验。 |
| E16.6 | E16.6.ag012_plan_v42_index_binding | 行为被改变 | Q26；§18.9 L2830–2832；E16.6 L1847 | `crates/evo-core/tests/support_scope.rs:1307–1466`：明确当前顶层 v4.2、旧谱系／recheck 集合／V098 入口；当前唯一入口改 v4.3、复核并集及 V120。 |
| E16.6 | E16.6.ag086_local_scoped_acceptance | 行为未变 | Q26；E16.6 L1843；§18.9 L2832 | `crates/evo-core/tests/support_scope.rs:1712–1740,1857–1914`：partial registry、execution 字段语法及旧事实不回填；具体当前版本绑定由 AG124 更新，不改此历史字段合同。 |
| E17 | E17.disabled_rejection_controller_acceptance | 范围外 | §18.9 L2830–2832；E17 L1851–1859 | `crates/evo-core/tests/e17_rejection_v41.rs:7–60,192–202`：默认关闭、不可自评分／改最终 grader；不在 §19.5 落点并集。 |
| E18 | E18.disabled_rejection_controller_acceptance | 范围外 | §18.9 L2830–2832；E18 L1865–1873 | `crates/evo-core/tests/e18_rejection_v41.rs:8–13,163–255,407–416`：默认关闭、保护路径／审批／隔离与自动合并拒绝；不在 §19.5 落点并集。 |

## 2026-10-12 第七阶段六小时候选与验收快照（AG-128）

本节是候选状态快照；实际 main 为 `6a5b5ebf5ef0f03dfd46fb9812f9ead11ba0be29`，本阶段新合并产品数为 0。以下 PR 均 draft、merged=false；状态最高 implemented_not_verified，二次验收 not_executed。本节未增加 verified 索引记录。具体 CI 外发确认待处理，登记与合并触发尚未执行；按实际 main 重放、重新验收最终 head 与合并树核对仍待完成。

### AG-124 / PR #165 — v4.3 派生绑定与历史复核（E16.6；V071–074/V080/V098 局部）
- 固定 head：`31de61ec81febd344c4dd5136258299d40f97b54`；候选父：`6a5b5ebf5ef0f03dfd46fb9812f9ead11ba0be29`。
- 首次技术验收：passed；八项真实退出 0；Rust 147 组 / 1842 过 / 0 败，Python 69 过。独立 Rust 2 过 / 0 败，实际父编译后 0 过 / 2 败；267 项索引篡改拒绝。
- 实现子范围：109 条旧记录逐条复核，18 移出、89 标 rechecked_v4.3、2 范围外保留；旧验证版本、摘要与执行事实保留，判定依据见前节逐条表。
- 不可声明：CI 外发登记已批准、所有旧原始输入已取得、E16.6 整包完成、二次验收、效果、收益或发行。
- 证据：out/audit-20260930/cards/AG-124.md；qa/inputs/codex-input-ag-124-31de61ec.json；codex-r7-acceptance.md 的 AG-124 节。

### AG-125 / PR #166 — 本地生产入口单写者守卫（E04/E08/E16.5；V110.a 局部）
- 固定 head：`4c79e26b68bfaa7f5a8b1fcac7287123477df6a9`；候选父：`31de61ec81febd344c4dd5136258299d40f97b54`。
- 首次技术验收：passed；八项真实退出 0；Rust 148 组 / 1849 过 / 0 败，Python 69 过，生产开库检查单测 16 过。独立 head 3 过 / 0 败，实际父编译后 0 过 / 3 败。
- 实现子范围：共享数据目录 OS 锁、只读门及锁内复核、三处生产 Store::open 机检；锁持有至显式关闭。R2 仅协调七个新增夹具，旧首次验收的 test_exit=101 保留。
- 不可声明：旧失败变成通过、V110.b/c 权限或时钟、全部平台/真实宿主/真实沙箱、二次验收、效果、收益或发行。
- 证据：out/audit-20260930/cards/AG-125.md 与 R2 卡；qa/inputs/codex-input-ag-125-4c79e26b.json；codex-r7-acceptance.md 的 AG-125 节。

### AG-126 / PR #167 — host_surface v1 重复项与 record 读门（E02/E16.4；V107.a 局部）
- 固定 head：`663c12e30e99a926cd5b39cfbcab6d635bea5b07`；候选父：`4c79e26b68bfaa7f5a8b1fcac7287123477df6a9`。
- 首次技术验收：passed；八项真实退出 0；Rust 150 组 / 1858 过 / 0 败，Python 69 过，生产开库检查单测 16 过。独立 head core 2 / engine 3 全过，实际父编译后对应 2 / 3 全败。
- 实现子范围：同名异义 v1 项拒绝、精确克隆保留、HostSurfaceRecord 类型反序列化须精确 v1 schema；新 prepare 与重载沿用已有角色/见证/manifest 守卫。R1 新 prepare 夹具为零 supported；schema 读门与 legacy 重载解释待二次验收裁决。
- 不可声明：完整 Q19 产品门、v2 typed/升级差异/V107 整族、真实宿主、二次验收、效果、收益或发行。
- 证据：out/audit-20260930/cards/AG-126.md 与 R1 卡；qa/inputs/codex-input-ag-126-663c12e3.json；codex-r7-acceptance.md 的 AG-126 节。

### AG-127 / PR #168 — 种子纯分类失败关闭（E16.3；V063/V115.c 局部）
- 固定 head：`92e1dffec1f651eab94f4b3db0249d04b6a9eef7`；候选父：`663c12e30e99a926cd5b39cfbcab6d635bea5b07`。
- 首次技术验收：pending；全新 target 正在运行。实施完整自测八项真实退出 0；Rust 151 组 / 1864 过 / 0 败，Python 69 过；固定 head 三项独立反例 3 过 / 0 败，实际父编译后 0 过 / 3 败。不得把实施自测替代固定 head 首次验收。
- 实现子范围：不完整 B/L/U 或双方身份不能确认时 Unknown；有效关系保留。旧 enum/wire 与持久 install/reset/撤销/自动激活代码未改；L/U 未确认取 Unknown 的最小解释待二次验收裁决。
- 不可声明：内置 catalog/I0 接线、可重算来源绑定、生产 classify 调用、完整 V115/E16.3、二次验收、效果、收益或发行。
- 证据：out/audit-20260930/cards/AG-127.md；qa/codex-r7-ag127/committed-head.json、fixed-head-probes-92e1dffe.json、accept-92e1dffe.log；codex-r7-acceptance.md。

P1 的可信时钟、统一撤销/预算闭包、管理取消、AG-121、摘要/许可/工具链/依赖，以及 P2/P3 和既有队列仍待推进。AG-111 台账重构未完成，本节不替代该卡。真实模型/付费实验/真实宿主与沙箱/历史 T 断言/E14 冻结合同只登记外部阻塞；货币预算 0。


## 第七阶段实际合并与首次技术验收更新（AG-128-R2）
前一节 34 行为 2026-10-12 当时的候选快照，按原字节保留；本节登记随后发生的事实，不覆盖旧 head、待批准状态或失败记录。AG-124/125/126/127 均已按固定 head admin --merge 合并并独立核对两个父提交与树；状态最高 implemented_not_verified，technical first acceptance passed，Claude 二次验收 not_executed。AG-111 台账重构与 AG-114 检查器增补仍未完成。

### AG-124 / PR #165 — v4.3 绑定与逐条历史复核
- 验收 head 0c3157f25079de5574ae7c2a9cc3adc01a529096；实际 merged 2c35d2e0f663064de2f5ee76c46154db7a50cf63；两个父提交与验收树一致。Rust 147 组 / 1842 过 / 0 败，八项退出 0、Python 69。原首次 109 条复核表记录 89 未变、18 撤出、2 范围外；本次更正再撤 AG054 一条，当前为 88 未变、19 撤出、2 范围外，90 条旧记录字段原样保留。
- 本会话已具体批准并登记现有 notify-merge 外发，未修改工作流、接收方、密钥或载荷范围；实际 webhook 送达未有证据，CI 登记强制核对仍待 Q24。登记格式待二次验收裁决。证据 out/audit-20260930/qa/codex-r7-ag124/merged-0c3157f2.json 与 codex-r7-acceptance.md 的 AG-124 节。

### AG124 逐记录复核更正 — E04.ag054_typed_predispatch

|记录|当前判定|依据与保留|
|---|---|---|
|E04.ag054_typed_predispatch|行为被改变，整条移出 verified_subscopes；此前 L8670 行为未变判定被本次更正取代|原 AG054 卡 L46–48/L70、accepted f6aa729 development_receipts.rs L933–960 与原日志 12/0 明确固化负 clock 后用旧 reserve 时间释放；违反 v4.3 §7.3.1 L1046。仅查 broker test_entry 曾遗漏完整已验行为，不得收窄 §18.9 复核。完整原对象、卡、输入及原 18 条撤出证据仍留本地。|
|E03.ag016_development_receipts|行为未变，保留原 rechecked_v4.3|原 accepted b1d008f 6 个测试无后来 AG054 的负 clock 补偿行为；仅新增义务不撤销未变旧行为。|

更正证据 out/audit-20260930/qa/codex-r7-ag128/ag124-ag054-clock-recheck-correction.json；不新增 AG129 时钟修补信用，其返修与完整数据库发生时间下界仍须独立验收。

### AG-125 / PR #166 — 实际合并软件子范围
- 最终验收 head ef2076d8e1eea7c406613c42369028c9bcaece26；实际父 2c35d2e0f663064de2f5ee76c46154db7a50cf63；merged 40c70df8e71e7d23a3f2ff373852451604b48ec4，mergedAt 2026-10-13T00:54:05Z；两个父提交与验收树一致。
- 首次技术验收八项退出 0，Rust 148 组 / 1849 过 / 0 败，Python 69；完整原日志、编译后父树语义反例及未通过旧记录见原回执与 codex-r7-acceptance.md。独立二次验收 not_executed。
- 索引只追加 E16.5.ag125_v43_single_writer；125/126唯一目标摘录来自原全量日志，非额外执行；127记录改用实际补跑的public-safe单目标6例，原151/1864八门只来自原first receipt。actual command含CARGO_TARGET_DIR且无--offline，八词offline command仅声明目标。
- E16.5; v4.3 §7.3.1 L1038–1046 and V110.a L2297, local single-writer subset only；No V110.b permissions or V110.c persistent clock floor; no all-platform, real host/sandbox, whole E/V, second acceptance, effects, benefits or release。证据 out/audit-20260930/qa/inputs/codex-input-ag-125-ef2076d8.json（SHA-256 20cfc8660225ae78426f450b1b3b6d1d5cf571b9cef70a776195f9a9c16cc51c）、out/audit-20260930/qa/codex-r7-ag128/excerpt-final-ag125-single-writer-ef2076d8.log。

### AG-126 / PR #167 — 实际合并软件子范围
- 最终验收 head d04a71c2a70b2266d33cb9a444bbdd3339121f55；实际父 40c70df8e71e7d23a3f2ff373852451604b48ec4；merged 1ce4c0fc00b46f72d91b7595bd7911a9f3627c67，mergedAt 2026-10-13T02:30:52Z；两个父提交与验收树一致。
- 首次技术验收八项退出 0，Rust 150 组 / 1858 过 / 0 败，Python 69；完整原日志、编译后父树语义反例及未通过旧记录见原回执与 codex-r7-acceptance.md。独立二次验收 not_executed。
- 索引只追加 E02.ag126_v43_host_surface_duplicates、E16.4.ag126_v43_host_surface_record；125/126唯一目标摘录来自原全量日志，非额外执行；127记录改用实际补跑的public-safe单目标6例，原151/1864八门只来自原first receipt。actual command含CARGO_TARGET_DIR且无--offline，八词offline command仅声明目标。
- E02; v4.3 §5.5.1 and V107.a L2273, local frozen-v1 duplicate subset only；No typed v2 surface, upgrades, supported consumer mapping, Q19 product gate, real host, whole E/V, second acceptance, effects, benefits or release。证据 out/audit-20260930/qa/inputs/codex-input-ag-126-d04a71c2.json（SHA-256 c25b421feedadbd646f70fd6778b0ce233595decd78722d24d2b80d15ad5eb0d）、out/audit-20260930/qa/codex-r7-ag128/excerpt-final-ag126-core-host-surface-d04a71c2.log。
- E16.4; v4.3 §5.5.1, E16.4 L1819–1827 and V107.a L2273, local record subset only；Real Claude Code host remains blocked; no typed v2, upgrade or complete Q19 gate, whole E/V, second acceptance, effects, benefits or release; schema reading and legacy reload interpretation remains pending second acceptance。证据 out/audit-20260930/qa/inputs/codex-input-ag-126-d04a71c2.json（SHA-256 c25b421feedadbd646f70fd6778b0ce233595decd78722d24d2b80d15ad5eb0d）、out/audit-20260930/qa/codex-r7-ag128/excerpt-final-ag126-engine-host-surface-d04a71c2.log。

### AG-127 / PR #168 — 实际合并软件子范围
- 最终验收 head c32affbf5d9a09c78ce754c3f4c8904ff64c09aa；实际父 1ce4c0fc00b46f72d91b7595bd7911a9f3627c67；merged 9faa0b529fdb600b8c0e1e6d6cc866d278889353，mergedAt 2026-10-13T03:43:29Z；两个父提交与验收树一致。
- 首次技术验收八项退出 0，Rust 151 组 / 1864 过 / 0 败，Python 69；完整原日志、编译后父树语义反例及未通过旧记录见原回执与 codex-r7-acceptance.md。独立二次验收 not_executed。
- 索引只追加 E16.3.ag127_v43_seed_classification；125/126唯一目标摘录来自原全量日志，非额外执行；127记录改用实际补跑的public-safe单目标6例，原151/1864八门只来自原first receipt。actual command含CARGO_TARGET_DIR且无--offline，八词offline command仅声明目标。
- E16.3; v4.3 §11.1.1 L1367, V063 L1955 and V115.c L2341, pure classification subset only；No production classify caller, seed catalog or I0 wiring, source digest recomputation, install/reset integration, whole E/V, second acceptance, effects, benefits or release; uncertain L/U classification interpretation remains pending second acceptance。证据 out/audit-20260930/qa/inputs/codex-input-ag-127-c32affbf-public-execution-r2.json（SHA-256 18fe078193d3956009aa42647f189c4dff4c5d81048b0a3ce939fb86dfec3da8）、out/audit-20260930/qa/codex-r7-ag127/public-execution-r2-c32affbf.log。

V107/V110/V115 与其余新增 V099–V120 场景族仍 planned；新增 related_evidence_refs 只导航上述局部事实，不形成整族信用。真实模型、付费实验、真实宿主/沙箱、历史 T 断言与 E14 冻结合同只登记外部阻塞；货币预算 0，未宣称效果、收益或发行。L/U 不确认分类及 HostSurfaceRecord schema/legacy 读门解释保留待二次验收裁决。

AG127原first receipt的实际command含本机绝对路径，R2公开卫生检查真实拒绝，原失败留qa/codex-r7-ag128/r2-final-preview-absolute-path-rejection.log。本次只用实际public-safe补充单目标执行input登记seed6，原c32回执与7369a561摘要原字节保留。补跑不是新的workspace八门，未改变生产代码或扩大验收。

AG128-R1 候选5402c317中的单目标补充实际命令仍携带绝对本机 PATH，旧私有扫描只覆盖用户目录而遗漏其它绝对根，不能作为可上传交付。原卡、实际补充、静态输出与全门启动材料全部保留。本R2仅采纳重新真实执行且无任何实际绝对本机路径的单目标补充；不事后改写旧字面执行。规定环境由私有guard配置，公开命令使用真实变量路径。

本R2另按底本公开卫生要求只节录23处既有真实本机路径，公开注明路径前缀略并给相对私有原文链接；保留所有任务/PR/SHA、计数、退出与失败事实。M4原8764行前缀不再宣称字节原样：唯一例外由 out/audit-20260930/qa/codex-r7-ag128/legacy-public-hygiene-r2/finite-line-sanitization.json 逐处列明，原完整M4 SHA05e45541264eaf964697c74a800fa5529ae8c69ef692f4b659793445b9505709、R1完整文 SHAdb4c98d7eb5a235657c279f9bf9fef040cafa2c6630b55f96b39f32cfb038dd6 先私有封存。原34行快照仍逐字保持，未关闭AG111。

R1候选5402c317全门因已证实公开路径阻塞被精确中止，实际guard143、source unchanged，结束2026-10-13T04:29:01.817159Z；原wrapper、partial与终止日志原样保留，固定头探针未执行。该候选未获全门首次验收，不继承partial8信用；R2新head全部门另跑。证据 out/audit-20260930/qa/codex-r7-ag128/final-main-wrapper-r1-5402c317.json 与 r1-known-blocking-hygiene-termination.json。


## 第七阶段实际合并与首次技术验收更新（AG-142）

本节追加 AG129/130 的实际合并事实，保留此前候选、失败、撤出、校正与计数原文。94 个既有证据对象逐条原样保留；只为 AG129 两个实际提交的 Rust 目标增加 E16.5 局部记录，当前共96条。最高状态 implemented_not_verified，首次技术验收 passed；Claude 二次验收 not_executed。AG111 台账无损重构与 AG114 检查器增补仍另卡未完成。

### AG-129 / PR #170 — 可信时钟输入与命名拒绝传播（E16.5；V110.c 局部）

- 验收 head `f2544a95cd8bbbea8ea9840af2b1afb4a4475117`；父 `58c013572c806dc464a248331aeca41bcc4226c3`；merged `89b3c20427e8b82f07ec470d9cd4f1435f0df7ac`，2026-10-13T12:11:17Z；实际两个父与树均已核对。
- 新鲜独立八门全部退出0；原日志计数153个结果摘要，准确分解148个 Running 测试可执行入口与5个 Doc-test 摘要，1887过/0败；Python checker69过与生产开库16过。相对父151摘要/1864过为净增2摘要/23用例，24新增具名、1旧用例移除；原计数不改写。独立同APP控制Head9过/父9编译后语义败，内退出101/guard0；源与支付事实保持。
- 新记录只覆盖 `clock_unix_v43` 的3过/0败（纪元零、负纪元命名拒绝、单位保持）与 `clock_fail_closed_v43` 的2过/0败（负时钟在首次保留和夹具派发前拒绝）。两个独占日志摘录逐字来自原独立完整 raw；没有追加执行或把其他旧目标计为新产品。执行对象沿用真实完整八门命令，记录级八词 offline 命令仅为声明目标。
- 输入 `out/audit-20260930/qa/inputs/codex-input-ag-129-f2544a95.json`，SHA256 `40e1ae8bdd3819503ddff9480ef18207439fdb42ec785d4fbf5b5fa55d16f718`；合并证据 `out/audit-20260930/qa/codex-r7-ag129/merged-f2544a95.json`；唯一摘录 `out/audit-20260930/qa/codex-r7-ag142/excerpt-ag129-clock-unix-f2544a95.log`（SHA256 `bacc87d088cb923be11a4c7ba59b83fe52dca9357dedb442a58ab9b47553cb98`）与 `excerpt-ag129-broker-first-clock-f2544a95.log`（SHA256 `2afca862b25f1ef69fb964645307e82e636ca96fc93fb017a9dd3fd7ed46e188`）。完整原始执行、9/9控制和历次未通过记录见同input及 `codex-r7-acceptance.md` AG129节。
- 不可声明完整数据库最大发生时间floor/whole V110.c、真实坏系统钟、物理零SQL、真实宿主/沙箱/提供商、全部E/V、效果收益发行或二次验收。formal typedwire/历史版本/逻辑快照解释保留“待二次验收裁决”。未确定和已付费用不因错误时钟自动释放。

### AG-130 / PR #171 — 构建输入静态检查（E16.5；V113.a/c 局部）

- 验收 head `89105e349271d3c867e19331325ec3c322647361`；父 `89b3c20427e8b82f07ec470d9cd4f1435f0df7ac`；merged `fbc8c5545760a17242c6ee588d6c9340d1d97300`，2026-10-13T19:19:12Z；两个父、树与Root main快进均实际核对。
- 七文件实现锁定工具链/CI action不可变引用与静态构建输入检查；独立新鲜八门全0，148 Running+5 Doc-test摘要/1887过/0败；checker69、构建输入额外50、生产开库16过。新增Rust产品测试0，所以未追加 AG130 verified_subscopes，也未借既有seeds或未提交APP制造Rust产品信用；只在 E16.5 实现/状态原因/未完事项中登记 implemented_not_verified 与首次技术验收。
- 固定APP Head实际编译2过/0败，原wrapper读取nocapture布局时退出1保留；严格对同原raw只读补读确认2过，不重跑Cargo、不改旧失败。实际父两编译语义败/0过，inner101/guard0、4PARSED/2CONTRACT/0INVALID，源不变。原 rust-objcopy/dyld 非fatal警告保留，无真实宿主信用。
- 输入 `out/audit-20260930/qa/inputs/codex-input-ag-130-89105e34.json`，SHA256 `0050a2e2ab79bbb006898cb2c1da0ccb310c068693f82d6af61ea7af29e7fb57`；合并补证 `out/audit-20260930/qa/codex-r7-ag130/actual-merge-and-local-FF-proof-fbc8c554.json`，SHA256 `82a678d86b5ba4049cca1a90f5eb24541d385b41bbe08a1623a77c3f0a5b361e`。原first receipt merged:null 为签发时快照，原字节保留；实际merge由补证承载。
- 合并后普通Git TLS传输多次退出128均保留；官方Git API的既有真实signed commit原始 payload/signature 经SHA1对象框架与独立无写Git核验精确得实际M7，才导入原对象、CAS origin与本地FF0。未合成合并提交、未关闭TLS验证或改代理配置；旧FETCH_HEAD未冒称新fetch成功。初次本地FF受沙箱锁写拒绝128，获权限后实际0；没有把拒绝记0。
- 不可声明hosted Actions执行、ZIP/archive/发行许可与完整依赖登记、whole V113/V116、真实宿主/沙箱、效果收益发行或二次验收。构建输入CLI/类型合同与同raw补读方式为“待二次验收裁决”。原已批准 notify-merge 的接收方/密钥/载荷范围未改，实际送达未独立证实。

V110/V113/V116与其余新增场景族仍 planned；related_evidence_refs 仅导航E16.5两个真实局部Clock记录，不形成整族信用。真实模型、付费E01/E11/E15、真实宿主E16.4/沙箱E16.5、历史包及T断言、E14冻结合同只登记外部阻塞；货币预算0，无真实提供商调用。AG131许可/AG133依赖登记及P1→P2→P3、既有119/120/121/122队列继续，不能将本台账交付当第七阶段完成。


## 2026-10-14 第七阶段周期台账：AG131/121/133（截至 PR #175）

底本§5的周期折入：前一台账 PR #172 于2026-10-13T21:13:01Z合并，此后#173/#174/#175均已有真实固定head首次技术验收与双父同树证明；本节只折入事实，不改变真源、旧记录或AG111历史重构范围。状态最高implemented_not_verified；Claude独立二次验收not_executed，货币预算0，无真实提供商调用。索引由96增至98条，96原对象和顶层field_semantics/plan/谱系/项目许可原字节保留；V111/V116及其余新场景族仍planned，related_evidence_refs仅导航有限子范围。

### AG-131 / PR #173 — 项目许可元数据与Rust登记守卫

- 固定验收head `4232a2265c70f8732fa13496603d18a3d2be88c6`，实际唯一父 `557fd81882123e2dc1f48e82c23f75e40c1c54a9`；merged `59c0af57ee0fec291b12d5a7dab3ba62ca726579`，时刻 `2026-10-14T00:50:19Z`；实际Git两父全等上述父与head，完整tree `1c371c9e9d82b9e9d1de24d1263121eb99e74dae` 等于验收head，Root已快进。
- 首次独立Controller八门全0；Rust 1889过/0败，153 summaries=148 Running+5 Doc-tests；support Python 81，build-input Python 50，production-store Python 16及checker0。全部来自实际新head完整日志，不继承旧head或self成绩；不是Claude二次验收。
- 原first input `out/audit-20260930/qa/inputs/codex-input-ag-131-4232a226.json`，SHA256 `2d0cfa8ad9e29ee4ccde8f0a3e472885dccf94fe10446e2b6de451849d2eaf31`；其merged:null/merged_at:null是签发时快照，原字节不补写。实际合并补证 `out/audit-20260930/qa/codex-r7-ag131/actual-merge-and-root-FF-proof-59c0af57-r4.json`，SHA256 `64fdceea5f45290fad8e2bbe27ae372a7c3932e011218e98111cdc3bbf3935d7`。
- 只新增 E16.6.ag131_v43_project_license_metadata：实际已提交support_scope目标25过/0败，其中新增两项许可元数据Rust守卫；从原独立全量raw逐字唯一摘录，不新跑、不把23旧例全部授V116.a。LICENSE/workspace/README机器元数据一致性为有限范围，不声称法律充分、完整上游通知或整V116/E16.6。
- Head两实际Rust反例绿，正常编译Parent两目标语义红101；文件机制十二项篡改只Head范围，不冒父编译证明。原a015失败clippy/test101与smokeCLI1、Python/探针解析错误、Git TLS/GraphQL/REST EOF及合并GET EOF原像保；见原input的deviations和 `out/audit-20260930/qa/codex-r7-ag131/actual-merge-and-root-FF-proof-59c0af57-r4.json` 的preserved_failures。没有关闭TLS或force。
- 不可声明：整E/V、效果收益发行、真实提供商/宿主/沙箱、付费E01/E11/E15、历史T等价、E14冻结合同或Claude二次验收；各卡的最小失败关闭细节仍待二次验收裁决。

### AG-121 / PR #174 — 脱敏bound调用的严格读取与身份保持

- 固定验收head `78ed65ece5aa52fcce7328333432ea1da20378c4`，实际唯一父 `59c0af57ee0fec291b12d5a7dab3ba62ca726579`；merged `a5039eb21558cb466a0ee00a1b2ba022b11c12d1`，时刻 `2026-10-14T08:18:42Z`；实际Git两父全等上述父与head，完整tree `84467ac529e67b6b94aaa9b2692d33175d94e175` 等于验收head，Root已快进。
- 首次独立Controller八门全0；Rust 1900过/0败，154 summaries=149 Running+5 Doc-tests；support Python 81，build-input Python 50，production-store Python 16及checker0。全部来自实际新head完整日志，不继承旧head或self成绩；不是Claude二次验收。
- 原first input `out/audit-20260930/qa/inputs/codex-input-ag-121-78ed65ec-pr174.json`，SHA256 `4d982ae988422295e7cc08b59cfdd2313cfaad6fe851b779e8c0c7bf73718236`；其merged:null/merged_at:null是签发时快照，原字节不补写。实际合并补证 `out/audit-20260930/qa/codex-r7-ag121/actual-pr174-merge-and-root-FF-proof-r2.json`，SHA256 `64860193972f2b94bae88629cb6dff084d86ac083bce494dfb2cb303cbcaf679`。
- 只新增一条E04.ag121_v43_redacted_bound_request_identity记录：已提交broker_redacted_bound_guards_v42目标11过/0败由原raw独占摘录登记，不制造第二次执行。AG121原first在E04/E08的11项实际测试与严格reader/原请求身份及逻辑关闭保持事实完整保留；E08只在implemented_not_verified、首验范围和remaining说明槽记录，不新增verified_subscope，V111仅导航E04记录。TransportNone、畸形body是kernel fixture；真实producer original_digest审计和whole V111.c仍由AG119等后续验收。
- Head正常编译六控绿；Parent正常编译两健康绿、四目标语义红101（legacy脱敏bound非Forbidden、畸形有效JSON/digest错误接受、不同binding或畸形体写恢复关闭）；原H841 self1898过/2败与test-only收据观察修正保。证据 `out/audit-20260930/qa/codex-r7-ag121/actual-self-8435-terminal-failure-r2.json`；Draft远端API EOF/ Git TLS错误保留actual-174-* R1/R2原文件，成功R3不覆盖失败，未放宽读取或回执门。
- AG149旧候选a700dfb1的暖结构检查先因真源符号链接引用退出1；改用唯一regular真源后再次退出1，准确拒绝同test_entry/source_sha跨E04/E08重复登记。原拒绝报告 `out/audit-20260930/qa/codex-r7-ag149/actual-pre-self-checker-a700dfb1-r1.json`，SHA256 `eb974fab878d20bbe01b86d485b50e306c454b9e258739961fc39a3e7b79f269`；原99条候选、R2/R3静态准备和旧H保留，均没有self/独立八门或首次验收。AG149-R1按既有去重守卫改为98条，跨任务与不同摘录的口径待二次验收裁决；不更改旧first、11项实际测试或验收判据。
- 不可声明：整E/V、效果收益发行、真实提供商/宿主/沙箱、付费E01/E11/E15、历史T等价、E14冻结合同或Claude二次验收；各卡的最小失败关闭细节仍待二次验收裁决。

### AG-133 / PR #175 — 完整依赖登记与主档许可元数据检查

- 固定验收head `cb909860c293b0bf5ccd98a3c3fae3aac7d3f80b`，实际唯一父 `a5039eb21558cb466a0ee00a1b2ba022b11c12d1`；merged `14f832e7bb9e321eafc38097538e9267183d5069`，时刻 `2026-10-14T10:16:04Z`；实际Git两父全等上述父与head，完整tree `b2f9a02d02929e4182d961ad93ff7cedb4306e8b` 等于验收head，Root已快进。
- 首次独立Controller八门全0；Rust 1900过/0败，154 summaries=149 Running+5 Doc-tests；support Python 81，build-input Python 58，production-store Python 16及checker0; dependency-register Python30。全部来自实际新head完整日志，不继承旧head或self成绩；不是Claude二次验收。
- 原first input `out/audit-20260930/qa/inputs/codex-input-ag-133-cb909860-pr175.json`，SHA256 `6c9c7ad68986cd9a9e295e198fa683285ee56b63931c8eda54f7f05742db756b`；其merged:null/merged_at:null是签发时快照，原字节不补写。实际合并补证 `out/audit-20260930/qa/codex-r7-ag133/actual-pr175-merge-and-root-FF-proof-r2.json`，SHA256 `b2b3eb98656512deee015ef61e012216cbaf9f4e7312d267af187a694bc38a70`。
- 已提交Rust产品测试新增0；不新增verified_subscope，不借私有未提交CPP或Python/CI成果授cargo目标信用。E16.6只用status_reason/implementation_files/remaining说明槽记录implemented_not_verified+首次技术验收，非Rust说明槽沿AG130模式，待二次验收裁决。真实登记22外部直接依赖/70使用边（61常规+9dev）及锁304包（298registry+6local），主档archive checksums和许可元数据为有限程序检查；不等于完整offlinekit、法律充分、完整notices或整V116。
- Head正常编译三控绿；Parent正常编译F00健康绿，D1删除rustix unix使用边与D2自洽伪MIT对rmcp主档Apache两首目标语义红101；每边3selected、ignored/measured0、filtered25、outer0，CPP不加入产品索引。原R2/R3/R4 archive/名字/解析失败、低8门拒绝、onecommit前资源停止和M9→M10逐字重叠证明均保。实际初push TLS128、Ready后API EOF1、readonly TLS1、首次admin EOF1不记0；先核确实未merged再同H重试，最终双父同树实证。
  - 过程原证 `out/audit-20260930/qa/codex-r7-ag133/actual-push-cb909860-first-TLS-failure-r1.json`，SHA256 `b4ce68baf254c361a75e6c918ee82af11c10c2878fd2ba729c467582a6d54cc7`。
  - 过程原证 `out/audit-20260930/qa/codex-r7-ag133/actual-175-ready-r1-EOF-failure.json`，SHA256 `ee2b5b4f678ca2742925854917ef816876372c4a1ace66d4caa4c38410eefdd6`。
  - 过程原证 `out/audit-20260930/qa/codex-r7-ag133/actual-175-readonly-r2-TLS-failure.json`，SHA256 `9e461120b0f6a451cda6e5ec5c6093e28257c51122d1a54bfc02883a06cb9463`。
  - 过程原证 `out/audit-20260930/qa/codex-r7-ag133/actual-175-admin-attempt1-EOF-r1.json`，SHA256 `c5b9da0c52509456df95be8e9ecd12926831ca7424794161612ccbcf0b6f3876`。
  - 过程原证 `out/audit-20260930/qa/codex-r7-ag133/actual-175-admin-attempt2-terminal-r1.json`，SHA256 `e23fd50e750a3992182d54ef629b902c5ebb1fa1d88a2bcbb8bf289a6dd6d9d3`。
- 不可声明：整E/V、效果收益发行、真实提供商/宿主/沙箱、付费E01/E11/E15、历史T等价、E14冻结合同或Claude二次验收；各卡的最小失败关闭细节仍待二次验收裁决。

非Rust实际本地验收（非hosted CI运行，且不新增E/V验收信用）：

|任务|实际命令|真实退出/计数|原日志|
|---|---|---|---|
|AG131|`python3 scripts/test_check_build_inputs.py`|0 / 50 tests OK|`out/audit-20260930/qa/codex-r7-ag131/accept-4232a226-build_inputs50.log`|
|AG131|`python3 -m unittest scripts/test_check_production_store_open.py`|0 / 16 tests OK|`out/audit-20260930/qa/codex-r7-ag131/accept-4232a226-production16.log`|
|AG121|`python3 scripts/test_check_build_inputs.py`|0 / 50 tests OK|`out/audit-20260930/qa/codex-r7-ag121/accept-78ed65ec-pr174-r1-build_inputs50.log`|
|AG121|`python3 -m unittest scripts/test_check_production_store_open.py`|0 / 16 tests OK|`out/audit-20260930/qa/codex-r7-ag121/accept-78ed65ec-pr174-r1-production16.log`|
|AG133|`python3 scripts/test_check_build_inputs.py`|0 / 58 tests OK|`out/audit-20260930/qa/codex-r7-ag133/accept-cb909860-pr175-r1-build_inputs58.log`|
|AG133|`python3 -m unittest scripts/test_check_production_store_open.py`|0 / 16 tests OK|`out/audit-20260930/qa/codex-r7-ag133/accept-cb909860-pr175-r1-production16.log`|
|AG133|`python3 scripts/test_check_dependency_register.py`|0 / 30 tests OK|`out/audit-20260930/qa/codex-r7-ag133/accept-cb909860-pr175-r1-dependency30.log`|

原notify-merge登记接收方/密钥/载荷范围不改变，不额外手动外发；送达未独立证实。真实模型、付费实验、真实宿主/沙箱、历史包/T、E14合同仍仅登记外部阻塞。P1→P2→P3及既有队列仍未全闭，本周期台账不是阶段完成。


## AG-156：Q24 非 Rust CI 与外发资格

状态上限为 implemented_not_verified。本节登记前端、Python、CI 的完整交付合同；非 Rust 命令不进入 cargo 单目标证据语法，new_verified_subscopes 为空。

- 独立前端作业、路径集合、固定 CI 入口及日志声明见 `.github/workflows/non-rust-delivery.yml` 与 `docs/non-rust-delivery-register.json`。入口 `python3 -B scripts/non_rust_delivery.py` 实际执行 `node --check`、`node --test`、既有 Python discover 和 build-input 检查；不安装 npm、不做浏览器或真实后端测试。
- 入库登记为静态合同；实际命令、数字退出码、完整 stdout/stderr 及可恢复原字节由每次运行回执承载。未启动记录为 null，必需命令失败或未跑即整体非零。`out/non-rust-delivery/` 是作业期间副本，持久日志以实际 CI run/attempt、job 名与 console 标记定位；没有 numeric job ID 时不虚构 job URL。Python discover 合法 MCP skip 保留，不冒称 MCP 已验。
- 检查器读取 `docs/ci-egress-register.json`，与全部工作流的实际外发步骤及已批准 notify 的触发、合并条件、接收方、事件范围、密钥名和完整来源像对应。缺登记、未批准改动、自报新 SHA 或新 entry 自确认均拒绝。既有三套 Rust 输入门、默认工具链与已批准通知范围保持；不新增第三方发送或 artifact 上传。
- 有限 workflow 外壳与步骤准入，以及既有 GitHub 仓库平台工件和第三方整合的分类，标记“待二次验收裁决”。本实现不宣称能静态判定任意 shell、提供网络沙箱或证明通知送达。
- 本地旧 M31 基线已复现：健康 0；移除 notify 登记和取消 merged 条件后自报新 SHA，两坏输入仍为 0。原证 `out/audit-20260930/qa/codex-r7-ag156/AG156-M31-Q24-baseline-actual-warm-r1.json`（SHA256 `59c2a9d91364a245d8cba8d55ba95016ad27a3230fb4ab896fcbf81ef57288ca`）。这只证明旧缺口，不是正常父编译 CPP 或新实现首次。
- 此登记写入时固定新 head 全八、真实两侧父编译语义反例、独立控制器和 hosted CI 尚未运行，退出码不预填。后续结果须以本任务私有实际首验/合并证明复算；最高状态不因此上调。

不声明整 E/V、完整许可或离线包、真实模型/付费实验/宿主/沙箱、历史 T 等价、效果收益、发行或 Claude 二次验收。货币预算0。


## 第八阶段 AG-182：v4.5 绑定与复核

本节登记 E16.6 增量 AG-182（PR #200）：派生索引、Python 检查器与 Rust 索引测试由 v4.3 直接绑定到当前 v4.5（v4.4 未经索引单独绑定，由本 PR 一并完成，§18.11），并按 §18.10 的 v4.5 机器口径（Q-50）完成并集 84 条记录的逐条复核、撤出清单、缺失清单、重跑登记与第七阶段补登。状态最高 implemented_not_verified；W0 首次验收（十一门、索引篡改探针、独立对抗审查）与管理员合并尚未发生，Claude/GPT 二次验收 not_executed。正文、归档与 out/ 下原始材料不上传；本节只登记文件名、摘要与相对路径。

合并事实（AG-200 回填）：PR #200 已由管理员合并，merged_sha `02a135aa918fce5dd2ea0c1c8eafc287318f10d2`，mergedAt 2026-10-25T04:39:38Z（合并提交的父提交 a389c4e3、11a2cbcc；PR head `11a2cbcc16715e4e0258dd64b38b87213662b600`）。上段“管理员合并尚未发生”与下文“pr = #200（未合并，merged_sha 为 null，事实表登记 200 => None）”是登记时的事实，原文保留；33 条 PR #200 记录的 merged_sha/pr_state 与两侧合并事实表由 AG-200 回填（见文末 AG-200 节）。

### 真源身份、谱系与动作合同

- 当前唯一入口：`RSIAgent-v4.5定稿-工程实施方案-2026-10-24.md`，SHA-256 `f6f9b1e44f3b1f364c0db8720f7eaec490ed7a03ddc053de8bfbab65326e6164`，544,031 字节，3,552 行；修订只来自 §19.7 的 Q-43–Q-60（2026-10-23 四段对抗复核 87 条发现，主控裁决），底稿 D44 = v4.4 定稿。
- v4.4：`RSIAgent-v4.4定稿-工程实施方案-2026-10-23.md`，SHA-256 `45ae29d71396f77dffc3baab7fed50564334734623bcec15f267778ddebeb496`，490,305 字节；已按原名原字节移入本地 `archive/2026-10-24-pre-v4.5/`，只作谱系，不作条款依据；修订 Q-27–Q-42（§19.6，Harness² 借鉴与 G1–G11 真源空白）。
- v4.3：`RSIAgent-v4.3定稿-工程实施方案-2026-10-12.md`，SHA-256 `73dd00bf5ef6e9757d0c391c9a8be4ca92b6e06a750aacb542817f82cbd208a7`，归档于本地 `archive/2026-10-23-pre-v4.4/`（AG-124/#165 的绑定对象）。
- 谱系 `v4.1→v4.2→v4.3→v4.4→v4.5`；历史记录保留其验证时的 plan_version/plan_sha256；本 PR 新执行的 33 条重跑/重验记录 plan_version 为 v4.5，并按 §18.5 显式写 `plan_recheck: not_affected`；12 条补登记录是 10-14～10-18 在 v4.3 下的首验，按主控裁决（审查 R-04 方案 ii）保留 plan_version v4.3 / `73dd00bf5ef6e9757d0c391c9a8be4ca92b6e06a750aacb542817f82cbd208a7` 并标 `plan_recheck: rechecked_v4.5`（plan_version 记录该验证当时所依据的方案版本；“新写入一律当前版本”针对新运行，不针对旧验证的补登）。
- 动作合同（§18.10/§18.11）：正文生成与 SHA、原件归档、修订清单与裁决记录已由 W0 在本地完成（v4.4：2026-10-23T20:58Z；v4.5：2026-10-24T01:58Z；裁决与工具留存本地 v44/、v45/）；派生索引、检查器与索引测试的版本绑定、谱系、plan_recheck、supersedes/completes、withdrawn_records、待重验清单与缺失清单由本 PR 完成；§16.8 读取范围与授权外研究动作的台账登记见下文。

### 机器口径落点（§18.10 v4.5 补充 Q-50；§2.9、§16.8；§13.5）

- 两套检查器同步：`scripts/check_support_scope.py` 与 `crates/evo-core/tests/support_scope.rs` 均绑定 v4.5 文件名/SHA 与五段谱系；`plan_recheck` 取值 not_affected、required、rechecked_v4.2、rechecked_v4.3、rechecked_v4.5（v4.4 从未单独绑定索引，rechecked_v4.4 不是合法标记），`rechecked_<版本>` 必须晚于记录自身 plan_version，且每条记录的应有值由两套检查器按记录推出（返修 R8-1，审查 R-01/R-06）：plan_version 为当前 v4.5 的记录必须显式带 not_affected（§18.5）；早于 v4.5 的并集任务记录必须为 rechecked_v4.5；并集外、属 v4.3 复核集合（除 E17/E18 外全部任务）且 plan_version 为 v4.1/v4.2 的记录必须保持 rechecked_v4.3（删除或改写即失败）；其余记录保持 not_affected（显式或缺省）；rechecked_v4.5 只允许出现在并集任务的记录上；复核并集 = §19.6（v4.5 更正后）∪ §19.7 落点按字面 = E00、E01、E02、E03、E04、E05、E06、E07、E08、E09、E10、E13、E16、E16.1、E16.3、E16.5、E16.6；并集外 E11、E12、E14、E15、E16.2、E16.4、E17、E18 的记录保留历史值，由上述规则强制（11 条 rechecked_v4.3、E17/E18 两条 not_affected、E16.4.ag126 一条缺省）。
- 记录可选字段 `supersedes`（只能指向 withdrawn_records 中 status=superseded 且同一 E 的条目）、`completes`（只能指向现存有效且同一 E 的记录，审查 R-08）、`note`；带 supersedes 或 completes 的记录 plan_version 必须 ≥ v4.5（这两个字段自 v4.5 才存在，审查 R-07）；顶层 `withdrawn_records`（id、withdrawn_in_pr、clause、owner_task、status ∈ pending_recheck/superseded/replaced_by_binding；24 个 (id, withdrawn_in_pr, clause, owner_task, status) 元组由两套检查器钉死，任一 id 不得再出现在 verified_subscopes；status=superseded 必须有记录 supersedes 它）；顶层 `evidence_missing_register` 钉 `reports/evidence-missing.json` 的路径与 SHA-256。field_semantics 追加 execution_runners、supersedes、completes、note、withdrawn_records、evidence_missing_register、plan_recheck、record_pins 八条语义（plan_recheck、record_pins 两条与 supersedes/completes/withdrawn_records/evidence_missing_register 中的同一 E、≥ v4.5、元组与三元组措辞为返修 R8-1 增补），原五条原文不变；execution.runner 追加 ctrl_verify_v43.sh、ctrl_verify_v45.sh。
- V080.d 三类机检（返修 R8-1 F2，审查 R-03）：(a) 上述 24 个撤出元组；(b) 32 个关系对（7 条 supersedes、25 条 completes，新记录 id → 旧记录 id），缺任一对或多出未登记的对均失败；(c) base a389c4e3 中保留的 93 条历史记录，各钉“去掉 plan_recheck 后的规范化 JSON（键排序、无多余空白、UTF-8）”的 SHA-256（Python `json.dumps(sort_keys=True, separators=(",", ":"), ensure_ascii=False)`，Rust `evo_core::canonical_fingerprint`，两侧逐字节一致），任一字段改写或记录消失即失败。本 PR 新写入的 45 条（33 条重跑/重验 + 12 条补登）本次不钉，由 PR #200 合并、merged_sha 回填后的下一个索引 PR 钉入（field_semantics.record_pins）。
- 缺失清单三种结果（§18.10“本地缺失的证据路径”）：登记且仍缺 → 报告字段 `evidence_missing` 列出（默认模式即无 `--evidence-root` 时，out/ 内的登记路径未经查验，改列 `evidence_registered_not_checked`，即“registered, not checked”，审查 R-12）；清单条目的 (record_id, field, path) 三元组（42 个）在两套检查器钉死，“删一条、另登一条、计数不变”（探针 c3）两侧均失败（返修 F5）；清单外缺失 → 失败；登记路径重新出现 → 登记了 SHA-256（及字节）则必须一致，登记为 unknown 一律失败（须经主控以其他绑定验真后另立新记录，不恢复旧证据）。out/ 是主检出未入库的本地证据库：克隆/worktree 不携带它，检查器只有在 `--evidence-root <主检出>` 给出时才对 out/ 路径的缺失判失败，否则只报告；out/ 外的路径（如 record_source 指向的本台账）任何时候都检查；`--known-missing` 默认 `reports/evidence-missing.json` 且须与清单指针一致。
- 两侧不对称（审查 R-05，主控登记为后续卡 AG-200）：Python 独有记录/根/traceability 的闭合键集、log_path 唯一、跨任务同一运行、input_ref 现存时的摘要、pr_state 与 completion 语义，以及 `--evidence-root` 下的本地证据库判定；Rust 独有清单路径固定为 `reports/evidence-missing.json`。缺省失败关闭与 Rust 证据库输入同属 AG-200，不在本 PR。
- V→E（§12 V 引用动词 Q-49：新增/补充断言计入，涉及不计入）：V121 = E03/E07/E09/E13（E16 统一追踪），V122 = E01/E07/E09（E16），V123 = E00/E16.5/E16.6（E16），V048 加 E05（Q-48），E06 对 V081 改为“涉及”而移出 V081 责任（Q-49；卡未列，以真源为准）。v_scenarios 增 V121–V123（planned、无执行信用）；trace_index_coverage：E00 = K01–K10、SO01–SO18、V087–V123、HS01–HS08、HSF01–HSF16、HSP01，E16.6 = B/U/K/SO/E 全集、V001–V123 与同三组 HS/HSF/HSP（V080 为 E00/E16.6 责任）。
- traceability 新增 `hs_borrowings`（HS01–HS08：正文小节、HSF 来源、E、V、adoption；HS05、HS06 为 registered_only = explicitly_out_of_scope，其余 adopted 但实现/验证均 planned；四列追踪按 §18.5 v4.5 补充）与 `hsf_sources`（HSF01–HSF16：`https://github.com/google-research/harness2`、commit `1cfe254e8d25b15f37048c3730134686abe60335`、tree `edc756a4d03a2826a248115482084a176e60e751`、路径、blob、Apache-2.0、read_range=ledger、E00/E16.5/E16.6；HSP01：arXiv:2610.14224v1 Section 2.2.1 与 Appendix F，无仓库对象）。卡写的 `google/harness2` 与真源 §16.8 地址不符，以真源为准。

### v4.4/v4.5 逐条复核表（并集 84 条；按 §18.11 一次重绑定同时完成两轮复核）

判定口径：只按适用的“改变实现行为=是”项判定；断言了被修订改变的旧行为 = 行为被改变（撤出并指派补全/修补任务）；只调用或触及但未断言该旧行为 = 相邻不矛盾；未触及 = 行为未变。依据中的源码行号为基线 main a389c4e3（M35）现行文件；结论按侦察表 recheck-table.json 与 recheck-delta.json 的 verdict_combined 执行，本卡未改判。行为未变与相邻不矛盾者均标 rechecked_v4.5。

| 记录 id | E | 适用 Q | 结论 | 依据 |
|---|---|---|---|---|
| E00.controller_acceptance | E00 | v4.4：Q-27否、Q-28否、Q-32是、Q-39否；v4.5：Q-56是、Q-59否 | 行为未变 | v4.4：记录验证固定基线定向回归（evaluator.rs:182–273：不完整行使票据失败且不退款、执行票据失配、无效统计先于完成拒绝）；适用Q项中唯一“是”的Q-32只改许可状态登记与README徽章，本记录未断言许可状态，Q-27/28/39为登记或现行已符，故不改变其行为。 v4.5：记录验证固定基线定向回归（evaluator.rs:182–273：不完整行使票据失败且不退款、执行票据失配、无效统计先于完成拒绝）；v4.5适用是项Q-56只涉README徽章/许可措辞机检与第三方定义，本记录无许可断言，Q-59仅登记，行为未改变。 |
| E00.ag099_step_future_default_test_stack | E00 | v4.4：Q-27否、Q-28否、Q-32是、Q-39否；v4.5：Q-56是、Q-59否 | 行为未变 | v4.4：记录验证step路径future装箱后默认2MiB测试栈下终态与决策语义不变（step_terminal_classes_v42.rs:1476–1960,2407–2547）；Q-32只涉许可状态/README，未被本记录断言，Q-27/28/39仅登记，行为未改变。 v4.5：记录验证step路径future装箱后默认2MiB测试栈下终态与决策语义不变（step_terminal_classes_v42.rs:1476–1960,2407–2547）；v4.5适用是项Q-56只涉README徽章/许可措辞机检与第三方定义，本记录无许可断言，Q-59仅登记，行为未改变。 |
| E01.controller_acceptance | E01 | v4.4：Q-30是、Q-34是；v4.5：Q-57是、Q-47是、Q-45是、Q-46是 | 相邻不矛盾 | v4.4：记录验证统计与留出合同静态子范围（evaluation_v41.rs:122–264,388–531）；Q-34只把可行性规划的等号与最终批准门对齐且最终批准门不变，Q-30只新增默认关闭的C子条件登记，本记录均未触及，行为未改变。 v4.5：记录验证统计与留出合同静态子范围：序贯UCB退化、alpha一次分配、锚点覆盖与C_simple/C_skillopt同起点合同（evaluation_v41.rs:191–209,372–490,596–635）；不调用decide()与可行性门，Q-57/Q-47未触及；Q-45把候选生成前登记扩到C_simple/C_skillopt，与所验同起点/先登记合同相邻不矛盾（缺每步候选数/重复次数为新增义务），Q-46不涉，行为未改变。 |
| E01.ag057_explicit_sample_registration | E01 | v4.4：Q-30是、Q-34是；v4.5：Q-57是、Q-47是、Q-45是、Q-46是 | 相邻不矛盾 | v4.4：记录验证显式样本量登记、越界拒绝与旧v1计划字节/摘要不变（evaluation_contract_v42.rs:60–123）；Q-30新增的C子条件登记属新义务、首轮仍A/B1/C，与所冻结v1 conditions字段相邻但不矛盾，Q-34不涉n校验，行为未改变。 v4.5：记录验证显式样本量登记、越界拒绝、旧v1计划字节/摘要不变及文档阈值（含max_regression=0.01）可装入早停计划（evaluation_contract_v42.rs:59–123,170–219）；不调用decide()与可行性门，Q-57/Q-47未触及（AG-192为固定样本引入max_regression时须保持冻结v1字节）；Q-45的c_subcondition_registry为新增登记，与冻结conditions相邻不矛盾，Q-46不涉，行为未改变。 |
| E02.controller_acceptance | E02 | v4.4：Q-37否、Q-38是、Q-40是；v4.5：Q-52是、Q-53是、Q-48否 | 行为未变 | v4.4：记录验证有界原子编辑/编译（skill_edit.rs:92–154,442–571：UTF-8前像、锚点、整包原子与确定性）；Q-38/Q-40只改管理操作未接线固定码与作业记录版本，本记录不涉，Q-37附着边为否，行为未改变。 v4.5：记录验证有界原子编辑/编译（skill_edit.rs:92–154,442–571）；Q-52/Q-53只改作业记录v2、管理作业终点固定码与已接线/平面错配判定，本记录不涉管理操作，Q-48仅登记，行为未改变。 |
| E02.ag055_v1_wire_goldens | E02 | v4.4：Q-37否、Q-38是、Q-40是；v4.5：Q-52是、Q-53是、Q-48否 | 行为未变 | v4.4：记录验证五个v1请求、Strategy与旧Evaluation的冻结字节及描述符扰动检出（v1_golden.rs:39–56,83–142,145–243）；Q-38/Q-40不改模型侧wire，Q-37为否，行为未改变。 v4.5：记录验证五个v1请求、Strategy与旧Evaluation的冻结字节及描述符扰动检出（v1_golden.rs:39–56,83–142,145–243）；Q-52/Q-53只改作业记录v2、管理作业终点固定码与已接线/平面错配判定，本记录不涉管理操作，Q-48仅登记，行为未改变。 |
| E02.ag065_host_surface_name_coverage | E02 | v4.4：Q-37否、Q-38是、Q-40是；v4.5：Q-52是、Q-53是、Q-48否 | 行为未变 | v4.4：记录验证宿主面名称反向覆盖（core:39–99、engine:299–314）；Q-38/Q-40与宿主面无关，Q-37为否，行为未改变。 v4.5：记录验证宿主面名称反向覆盖（core:39–99、engine:299–314）；Q-52/Q-53只改作业记录v2、管理作业终点固定码与已接线/平面错配判定，本记录不涉管理操作，Q-48仅登记，行为未改变。 |
| E02.ag092_local_scoped_acceptance | E02 | v4.4：Q-37否、Q-38是、Q-40是；v4.5：Q-52是、Q-53是、Q-48否 | 行为未变 | v4.4：记录验证P/B输入已知直接依赖撤销不可被Set/Reset隐藏（baseline_dependency_revocation_v42.rs:196–315,428–512）；所涉为摘要依赖边，Q-37附着边定义为否且现行已按严格条件，Q-38/Q-40不涉，行为未改变。 v4.5：记录验证P/B输入已知直接依赖撤销不可被Set/Reset隐藏（baseline_dependency_revocation_v42.rs:196–315,428–512）；Q-52/Q-53只改作业记录v2、管理作业终点固定码与已接线/平面错配判定，本记录不涉管理操作，Q-48仅登记，行为未改变。 |
| E02.ag093_local_scoped_acceptance | E02 | v4.4：Q-37否、Q-38是、Q-40是；v4.5：Q-52是、Q-53是、Q-48否 | 行为未变 | v4.4：记录验证正式编译P/B输入解析、权威见证与typed版本绑定（compiler_input_binding_v42.rs:41–155,158–335）；Q-38/Q-40不涉，Q-37为否，行为未改变。 v4.5：记录验证正式编译P/B输入解析、权威见证与typed版本绑定（compiler_input_binding_v42.rs:41–155,158–335）；Q-52/Q-53只改作业记录v2、管理作业终点固定码与已接线/平面错配判定，本记录不涉管理操作，Q-48仅登记，行为未改变。 |
| E02.ag126_v43_host_surface_duplicates | E02 | v4.4：Q-37否、Q-38是、Q-40是；v4.5：Q-52是、Q-53是、Q-48否 | 行为未变 | v4.4：记录验证冻结host_surface v1拒绝同名不同内容、保留完全相同克隆与golden字节（host_surface_duplicates_v43.rs:27–114）；Q-38/Q-40不涉宿主面，Q-37为否，行为未改变。 v4.5：记录验证冻结host_surface v1拒绝同名不同内容、保留完全相同克隆与golden字节（host_surface_duplicates_v43.rs:27–114）；Q-52/Q-53只改作业记录v2、管理作业终点固定码与已接线/平面错配判定，本记录不涉管理操作，Q-48仅登记，行为未改变。 |
| E03.controller_acceptance | E03 | v4.4：Q-27否、Q-29是；v4.5：Q-43是、Q-45是、Q-44否 | 相邻不矛盾 | v4.4：记录验证程序fixture下G1单步消费者、journal与开发选择（optimization.rs:496–728），其开发报告自标Fixture出处、只证程序路径；Q-29的identity_only拒作Skill证据与HS07另计参数截断均为新增义务，与本记录相邻但不矛盾，行为未改变。 v4.5：记录验证程序fixture下G1单步消费者、journal与开发选择（optimization.rs:496–728）；Q-43/Q-45不触及（v4.4 Q-29的adjacent保留）。 |
| E03.ag016_development_receipts | E03 | v4.4：Q-27否、Q-29是；v4.5：Q-43是、Q-45是、Q-44否 | 相邻不矛盾 | v4.4：记录验证开发执行/评分收据schema、登记纯函数runner与E03观察门（development_receipts.rs:451–828,1279–1438），断言的是身份、收据、账本与撤销收口且两侧分数恒等；v4.4只禁止把identity_only结果当Skill内容证据，本记录未如此断言，属相邻不矛盾。 v4.5：记录验证开发收据schema、登记纯函数runner与E03观察门（development_receipts.rs:451–828,1279–1438）；Q-45在既有登记/角色分离核验上新增candidate_consumption核验，相邻不矛盾。 |
| E03.ag079_local_scoped_acceptance | E03 | v4.4：Q-27否、Q-29是；v4.5：Q-43是、Q-45是、Q-44否 | 行为未变 | v4.4：记录验证复制诊断RuleHypothesis前的结构拒绝（diagnosis_hypothesis_structure_v42.rs:368–447,462–490）；其truncated为宿主投影截断而非HS07所改的轨迹覆盖计数，Q-29不涉执行器，行为未改变。 v4.5：记录验证复制诊断RuleHypothesis前的结构拒绝；Q-43/Q-45不触及。 |
| E04.controller_acceptance | E04 | v4.4：Q-27否、Q-33否、Q-40是、Q-41是；v4.5：Q-54是、Q-52是、Q-51否、Q-27否 | 行为未变 | v4.4：记录验证持久根预算预留/dispatch/结算与取消已派发不退款（budget.rs:74–410,456–517）；Q-41只界定时钟下界取哪些发生时间列、Q-40只改作业记录版本，本记录未断言二者，Q-33为否，行为未改变。 v4.5：记录验证持久根预算预留/dispatch/结算与取消已派发不退款（budget.rs:74–410,456–517）；未断言参与时钟下界的列集合或installed_on不参与（M35无该下界），Q-54维持原结论；所涉取消/清理不属管理作业记录，Q-52不涉，Q-51/Q-27仅登记，行为未改变。 |
| E04.ag082_local_scoped_acceptance | E04 | v4.4：Q-27否、Q-33否、Q-40是、Q-41是；v4.5：Q-54是、Q-52是、Q-51否、Q-27否 | 行为未变 | v4.4：记录验证根预算授权在自有事件中止时整体回滚并可同请求恢复（budget_sql_fault_rollback_v42.rs:584–592）；Q-40/Q-41不涉，Q-33为否，行为未改变。 v4.5：记录验证根预算授权在自有事件中止时整体回滚并可同请求恢复（budget_sql_fault_rollback_v42.rs:584–592）；未断言参与时钟下界的列集合或installed_on不参与（M35无该下界），Q-54维持原结论；所涉取消/清理不属管理作业记录，Q-52不涉，Q-51/Q-27仅登记，行为未改变。 |
| E04.ag077_local_scoped_acceptance | E04 | v4.4：Q-27否、Q-33否、Q-40是、Q-41是；v4.5：Q-54是、Q-52是、Q-51否、Q-27否 | 行为未变 | v4.4：记录验证typed E16预算来源快照、引用丢失后的费用与清理（e16_budget_binding_v42.rs:140–243，library-only）；Q-40/Q-41不涉，Q-33为否，行为未改变。 v4.5：记录验证typed E16预算来源快照、引用丢失后的费用与清理（e16_budget_binding_v42.rs:140–243，library-only）；未断言参与时钟下界的列集合或installed_on不参与（M35无该下界），Q-54维持原结论；所涉取消/清理不属管理作业记录，Q-52不涉，Q-51/Q-27仅登记，行为未改变。 |
| E04.ag121_v43_redacted_bound_request_identity | E04 | v4.4：Q-27否、Q-33否、Q-40是、Q-41是；v4.5：Q-54是、Q-52是、Q-51否、Q-27否 | 行为未变 | v4.4：记录验证脱敏bound请求严格读取、身份保持与同身份恰好一次关闭（broker_redacted_bound_guards_v42.rs:1240–1500,1757–1786）；该关闭即v4.4§11.3.1窄豁免（Q-33为否、现行实现不改），Q-40/Q-41未被断言，行为未改变。 v4.5：记录验证脱敏bound请求严格读取、身份保持与同身份恰好一次关闭（broker_redacted_bound_guards_v42.rs:1240–1500,1757–1786）；未断言参与时钟下界的列集合或installed_on不参与（M35无该下界），Q-54维持原结论；所涉取消/清理不属管理作业记录，Q-52不涉，Q-51/Q-27仅登记，行为未改变。 |
| E05.controller_acceptance | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 相邻不矛盾 | v4.4：记录验证程序fixture流式评测：真实receipt、乱序连续前缀、停机阻断、费用/曝光保留（streaming_evaluator.rs:748–1067,1242–1459）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，行为未改变。 v4.5：记录验证程序fixture流式评测：真实receipt、乱序连续前缀、停机阻断、费用/曝光保留（streaming_evaluator.rs:748–1067,1242–1459）；完整批用例经finalize_complete间接调用decide()但为正效应，未断言LCB<0⇒regressed分支（1010–1063），与Q-57相邻不矛盾；不涉可行性门，行为未改变。 |
| E05.ag052_versioned_batch_verdicts | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 行为被改变（撤出） | v4.4：记录验证显式控制v2保留统计判定、v1固定字节与理由（最终批准门后处理）（streaming_verdicts_v42.rs:815–958,1085–1164）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，行为未改变。 v4.5：记录验证显式控制v2保留统计判定、v1冻结字节/摘要/理由（streaming_verdicts_v42.rs:829–901）；其v1后处理用例把decide()在d=−1时的Regressed+negative_lcb作为预期原始判定与存储理由（858–864,882–895），正是v4.5 Q-57改变且纠正前不作依据的退化分支，须撤出并在AG-192纠正decide()后按v4.5重验。 |
| E05.ag083_partition_admission_rejection_atomicity | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 行为未变 | v4.4：记录验证分区准入拒绝不留部分成员/家族写入（partition_admission_atomic_v42.rs:165–398,771–851）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，行为未改变。 v4.5：记录验证分区准入拒绝不留部分成员/家族写入（partition_admission_atomic_v42.rs:165–398,771–851）；不调用decide()与可行性门，Q-57/Q-47未触及，Q-44/Q-48仅登记，行为未改变。 |
| E05.ag080_local_scoped_acceptance | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 行为被改变（撤出） | v4.4：记录验证QueryBook/Evaluator冻结计划绑定与失配前拒绝（evaluator_plan_binding_v42.rs:136–317,452–650）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，行为未改变。 v4.5：记录验证QueryBook/Evaluator冻结计划绑定与失配前拒绝（evaluator_plan_binding_v42.rs:136–317,452–650）；同一目标的旧wire逐字节golden（701–776）冻结了regression场景下decide()的Regressed+negative_lcb输出，属以Q-57所改分支结论作正向断言（字节级），须撤出并在AG-192纠正后按v4.5重验（重验时该golden随退化判据/理由更新或剔除该场景）。 |
| E05.ag087_local_scoped_acceptance | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 相邻不矛盾 | v4.4：记录验证正式冻结候选直接依赖与撤销绑定（formal_candidate_dependency_binding_v42.rs:923–1256,1416–1494）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，Q-37为否，行为未改变。 v4.5：记录验证正式冻结候选直接依赖与撤销绑定（formal_candidate_dependency_binding_v42.rs:923–1256,1416–1494）；完整批终结经finalize_complete间接调用decide()但为正效应、只断言Improved，与Q-57相邻不矛盾；不涉可行性门，行为未改变。 |
| E05.ag088_local_scoped_acceptance | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 相邻不矛盾 | v4.4：记录验证完整批报告绑定实际统计、篡改均值拒绝（complete_batch_statistics_binding_v42.rs:1062–1081）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，行为未改变。 v4.5：记录验证完整批报告绑定实际统计、篡改均值拒绝（complete_batch_statistics_binding_v42.rs:1062–1081）；其完整批经finalize_complete间接调用decide()但均值为1，未断言LCB<0⇒regressed，与Q-57相邻不矛盾；不涉可行性门，行为未改变。 |
| E05.ag089_local_scoped_acceptance | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 相邻不矛盾 | v4.4：记录验证早停报告绑定实际首个证书与迟到费用（early_stop_certificate_binding_v42.rs:1271–1465,1531–1655）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，行为未改变。 v4.5：记录验证早停报告绑定实际首个证书与迟到费用（early_stop_certificate_binding_v42.rs:1271–1465,1531–1655）；同目标完整批用例经finalize_complete间接调用decide()但为正效应，未断言退化分支，与Q-57相邻不矛盾；不涉可行性门，行为未改变。 |
| E05.ag090_local_scoped_acceptance | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 行为未变 | v4.4：记录验证invalid理由绑定实际Some-ticket付费事实（invalid_formal_reason_binding_v42.rs:1225–1301）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，行为未改变。 v4.5：记录验证invalid理由绑定实际Some-ticket付费事实（invalid_formal_reason_binding_v42.rs:1225–1301）；不经decide()与可行性门，Q-57/Q-47未触及，行为未改变。 |
| E05.ag091_local_scoped_acceptance | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 行为未变 | v4.4：记录验证迟到grader错误不撤销已确认早停（late_grader_stop_preservation_v42.rs:1115–1197,1411–1561）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，行为未改变。 v4.5：记录验证迟到grader错误不撤销已确认早停（late_grader_stop_preservation_v42.rs:1115–1197,1411–1561）；早停走§3.4.1序贯界、不经decide()，Q-57/Q-47未触及，行为未改变。 |
| E05.ag095_canonical_grader_receipt_digest | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 相邻不矛盾 | v4.4：记录验证外部宿主权威的独立grader收据规范摘要与伪造拒绝（M35经#187插入F02用例，所引两函数体与AG-124基线逐字相同）（formal_external_host_authority_v42.rs:1634–1819）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，行为未改变。 v4.5：记录验证外部宿主权威的独立grader收据规范摘要与伪造拒绝（formal_external_host_authority_v42.rs:1634–1819）；终结经finalize间接调用decide()但为正效应、只断言improved，与Q-57相邻不矛盾；不涉可行性门，行为未改变。 |
| E05.ag101_streaming_verdicts_v42 | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 行为被改变（撤出） | v4.4：记录验证v2完整批仅当UCB<0才确认退化（退化门，不是可行性门）（streaming_verdicts_v42.rs:959–1048）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，行为未改变。 v4.5：记录验证v2完整批“仅当UCB<0才确认退化”（streaming_verdicts_v42.rs:958–1047）；其用例以decide()的LCB<0且均值<0⇒Regressed为原始判定（962–963,997–998,1030–1031），确认阈值又为0而非v4.5的−max_regression，属v4.5 Q-57改变的行为，须撤出并在AG-192纠正后按v4.5重验。 |
| E05.ag109_external_host_authority_bound_ticket | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 相邻不矛盾 | v4.4：记录验证bound票据权威签发、附着环通过与其他边之环拒绝（所引函数体与AG-124基线逐字相同）（formal_external_host_authority_v42.rs:2012–2298）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划；所涉附着环正是Q-37（否，现行已按严格条件）的内容，行为未改变。 v4.5：记录验证bound票据权威签发、附着环通过与其他边之环拒绝（formal_external_host_authority_v42.rs:2012–2298）；终结经finalize间接调用decide()但为正效应，与Q-57相邻不矛盾；不涉可行性门，行为未改变。 |
| E05.ag100_formal_candidate_dependency_binding_v42 | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 相邻不矛盾 | v4.4：记录验证任一控制schema缺候选时签发前拒绝（formal_candidate_dependency_binding_v42.rs:1746–1856）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，Q-37为否，行为未改变。 v4.5：记录验证任一控制schema缺候选时签发前拒绝（formal_candidate_dependency_binding_v42.rs:1746–1856）；同目标终结用例经finalize_complete间接调用decide()但为正效应、只断言Improved，与Q-57相邻不矛盾；不涉可行性门，行为未改变。 |
| E05.ag118_late_receipts_after_final_report_v42 | E05 | v4.4：Q-27否、Q-34是、Q-37否；v4.5：Q-57是、Q-47是、Q-44否、Q-48否 | 相邻不矛盾 | v4.4：记录验证最终报告后迟到收据只留审计、报告字节不变（#197后所引函数体未变）（late_grader_after_final_report_v42.rs:1685–1773）；Q-34只把可行性规划与最终批准门在等号处对齐且最终批准门不变，本记录不涉可行性规划，行为未改变。 v4.5：记录验证最终报告后迟到收据只留审计、报告字节不变（late_grader_after_final_report_v42.rs:1685–1773）；完整批端点经finalize_complete间接调用decide()但为正效应、只断言Improved，与Q-57相邻不矛盾；不涉可行性门，行为未改变。 |
| E06.ag084_local_scoped_acceptance | E06 | v4.4：—（v4.4 并集外）；v4.5：Q-38是、Q-49否 | 相邻不矛盾 | v4.5：记录验证存储候选与阶段事实审阅不授予批准（candidate_review.rs:707–1231,2271–2380）；其调用的是同步只读审阅路径，且断言零写入、角色/命名空间限制与隐藏材料不泄露，均与v4.5 Q-53/Q-38同向，未断言not_wired/未知操作或平面错配旧行为，属相邻不矛盾。 |
| E06.ag116_edit_compiled_produce_review_v42 | E06 | v4.4：—（v4.4 并集外）；v4.5：Q-38是、Q-49否 | 相邻不矛盾 | v4.5：记录验证生产写入的EditCompiled事实经review_stored得精确diff、单字节改动被拒（edit_compiled_produce_review_v42.rs:715–882）；调用同步只读审阅路径但不涉已接线定义、平面错配码或只读HTTP入口登记，与Q-53相邻不矛盾，行为未改变。 |
| E07.controller_acceptance | E07 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-46是 | 行为未变 | v4.4：记录验证本地协议认证/身份/幂等与禁用模型范围（service.rs:178–426）；Q-29要求E07真实链用projection_consumed执行器、Q-30为默认关闭新功能，本记录不声明真实链也不涉多候选，行为未改变。 v4.5：Q-46只新增默认关闭方法的启用/停用事实与登记行，本记录未触及。 |
| E07.ag015_curriculum_step_consumer | E07 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-46是 | 行为未变 | v4.4：记录验证curriculum.step管理消费者、幂等探针调度收据与读侧重核（dispatch_management.rs:1235–1757）；其中“probe”是课程探针、纯函数只作课程夹具，Q-29/Q-30均未被触及，行为未改变。 v4.5：Q-46只新增默认关闭方法的启用/停用事实与登记行，本记录未触及。 |
| E07.ag017_exploration_start_consumer | E07 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-46是 | 行为未变 | v4.4：记录验证exploration.start的幂等世界登记与读侧纯决策重算（dispatch_management.rs:2290–3071）；不涉开发执行器或多候选合成，行为未改变。 v4.5：Q-46只新增默认关闭方法的启用/停用事实与登记行，本记录未触及。 |
| E07.ag027_management_cleanup_closure | E07 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-46是 | 行为未变 | v4.4：记录验证管理作业与私有输入进入撤销清理闭包、撤销依赖提交前拒绝（management_cleanup_v42.rs:1828–2031,3177–3445）；纯函数执行器只作产生收据的夹具，正是v4.4允许identity_only证明的收口链，行为未改变。 v4.5：Q-46只新增默认关闭方法的启用/停用事实与登记行，本记录未触及。 |
| E07.ag044_submit_upstream_gate | E07 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-46是 | 行为未变 | v4.4：记录验证提交时上游闭包撤销拒绝且零写入、同key回放原作业（submit_upstream_gate_v42.rs:1811–1880,2152–2342,2407–2538）；执行器只作闭包夹具，Q-29/Q-30未触及，行为未改变。 v4.5：Q-46只新增默认关闭方法的启用/停用事实与登记行，本记录未触及。 |
| E07.ag108_candidate_review | E07 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-46是 | 行为未变 | v4.4：记录验证EditCompiled审阅仅在producer原规则摘要下给出精确diff（candidate_review.rs:1523–1782，#188后所引函数体与基线逐字相同）；Q-29/Q-30不涉，行为未改变。 v4.5：Q-46只新增默认关闭方法的启用/停用事实与登记行，本记录未触及。 |
| E08.controller_acceptance | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证离线撤销即时封锁、分页清理、不可逆费用保留与独立锚恢复（M35行号经#198平移，函数体与AG-124基线逐字相同）（lifecycle.rs:305–518,1107–1318,1488–1546）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag036_revocation_source_collision | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证另一kind墓碑上的二次撤销返回命名Conflict/NotFound且零写入（revocation_source_collision.rs:262–307,312–365）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag035_cleanup_fixpoint | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证清理在闭包不动点才Complete、未知对象失败、重启续清（cleanup_fixpoint.rs:235–342,586–775）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag038_budget_revoke_gate | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证撤销来源不再预留/派发、已派发结算照记（budget_revoke_gate.rs:345–500,585–726,779–913）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag043_late_response_revoked | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证撤销后结算照记费、响应封锁、传输脱敏（late_settlement_revoked.rs:1–18）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag045_late_stage_fact_redaction | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证撤销后写入的阶段事实以rsia.redacted.v1存储（late_stage_fact_redaction_v42.rs:1385–1515,1518–1563,2149–2156）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag053_bounded_dependency_snapshots | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证依赖快照递归有界且语义/指纹不变（local_export.rs:135–199,304–611）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag068_native_authority_cleanup | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证登记原生权威后继在新撤销作业中逻辑清理（import_source_successor_cleanup_v42.rs:374–392,395–505,796–864）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag069_local_scoped_acceptance | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证Failed清理整前沿Admin重试与Worker续清（failed_cleanup_resume_v42.rs:248–463,888–1183）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag073_local_scoped_acceptance | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证已完成缓存对来源撤销拒绝且不重付（broker_cache_revoke_v42.rs:376–610,676–706）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免）；补close属已撤出的AG097，不计入本记录，行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag075_local_scoped_acceptance | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证typed导入来源/结果依赖只读绑定（library-only）（dependency_read_v42.rs:35–153,155–243）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag096_unreadable_bound_request_isolated | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证不可读历史预算请求单行隔离而不回滚命名空间（cleanup_unreadable_bound_request_v42.rs:698–842,972–1054）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag107_budget_ref_missing_fail_closed_v42 | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证预算引用丢失按存储闭包失败关闭（budget_ref_missing_fail_closed_v42.rs:518–830）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag106_budget_startup_recovery_v42 | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证启动时为崩溃丢失close的已结算调用补关闭一次、第二遍不写（budget_startup_recovery_v42.rs:565–822,899–958）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免）；该补关闭属已结算调用，与Q-33窄豁免方向一致，行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag104_restore_prefix_migration_upgrade_v42 | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证严格前缀旧迁移备份在私有副本升级、异常隔离（restore_prefix_upgrade_v42.rs:835–924,969–1046）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E08.ag117_formal_call_revoke_cleanup_v42 | E08 | v4.4：Q-33否；v4.5：Q-51否 | 行为未变 | v4.4：记录验证清理按登记来源引用判定正式调用闭包（formal_call_revoke_cleanup_v42.rs:619–856,1204–1268）；E08适用的唯一Q项Q-33为“否”（现行broker实现不改，仅为已结算调用执行关闭设窄豁免），行为未改变。 v4.5：v4.5 只适用 Q-51（否：执行关闭写集即现状），维持 v4.4 结论。 |
| E09.controller_acceptance | E09 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-45是、Q-46是、Q-44否 | 相邻不矛盾 | v4.4：记录验证确定性程序fixture探索协调、调用既有单步消费者、历史与中间节点拒发布（exploration_v41.rs:495–1234），fixture开发报告被标fixture_declared不作有效候选；与Q-29同语义区且同向，Q-30不涉，属相邻不矛盾。 v4.5：Q-45/Q-46只新增字段登记、runner合同核验与C_probe_compose启用事实；本记录未触及。 |
| E09.ag021_skill_group_jobs | E09 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-45是、Q-46是、Q-44否 | 相邻不矛盾 | v4.4：记录验证§7.7每作业至多两组、组间隔离与组合仅作标记且须完整重跑与独立验收（skill_groups_v42.rs:863–1028,1271–1301）；与Q-30合成须重新评估的方向一致、未禁止合成，开发报告为fixture，属相邻不矛盾。 v4.5：Q-45/Q-46只新增字段登记、runner合同核验与C_probe_compose启用事实；本记录所验内容与之相邻不矛盾。 |
| E09.ag024_exploration_cleanup_edges | E09 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-45是、Q-46是、Q-44否 | 行为未变 | v4.4：记录验证探索节点/派发事实/历史首写同事务写世界依赖边与脱敏读拒绝（exploration_cleanup_v42.rs:1187–1200,1514–1725,1769–1937）；不涉开发证据资格或多候选，行为未改变。 v4.5：Q-45/Q-46只新增字段登记、runner合同核验与C_probe_compose启用事实；本记录未触及。 |
| E09.ag026_practice_attempt_sets | E09 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-45是、Q-46是、Q-44否 | 相邻不矛盾 | v4.4：记录验证§8.5同题对比练习K=1缺省、K=3须登记且只授权一集、尝试不计独立样本或周期（practice_v42.rs:957–1212,2425–2764）；其纯函数/对比执行只服务练习登记、明确不作开发周期，与Q-29/Q-30相邻但不矛盾。 v4.5：Q-45/Q-46只新增字段登记、runner合同核验与C_probe_compose启用事实；本记录所验内容与之相邻不矛盾。 |
| E09.ag033_run_next_trusted_gate | E09 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-45是、Q-46是、Q-44否 | 行为被改变（撤出） | v4.4：记录验证run_next只把经E03门核验的开发证据记为Valid（exploration_trust_v42.rs:1497–1803,2194–2315），但其正向路径把RegisteredPureFunction出处、输出不依赖Skill内容的执行结果固定为Trusted节点质量与最终候选资格；v4.4§6.7.4规定identity_only结果不得作Skill内容开发证据，故该正向断言属被改变的旧行为，须撤出重验（fixture/伪造/篡改拒绝部分方向不变）。 v4.5：Q-45/Q-46只新增字段登记、runner合同核验与C_probe_compose启用事实；本记录所验内容与之相邻不矛盾。 |
| E09.ag039_legal_actions | E09 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-45是、Q-46是、Q-44否 | 相邻不矛盾 | v4.4：记录验证Deepen只授Trusted、已展开父不重复加深、最佳祖先规则（exploration_legal_actions_v42.rs:1656–1895），规则以标签为输入、v4.4未改；但Trusted来源夹具与E09.ag033同为identity_only，属相邻（夹具依赖），Q-29落地时须换投影消费替身后再跑。 v4.5：Q-45/Q-46只新增字段登记、runner合同核验与C_probe_compose启用事实；本记录所验内容与之相邻不矛盾。 |
| E09.ag040_exploration_registration | E09 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-45是、Q-46是、Q-44否 | 行为未变 | v4.4：记录验证注册指纹只覆盖不可变字段、计数与成本由真实派发事实反推（exploration_start_after_dispatch_v42.rs:1146–1331,1664–1792,2410–2474）；不涉证据资格或多候选，行为未改变。 v4.5：Q-45/Q-46只新增字段登记、runner合同核验与C_probe_compose启用事实；本记录未触及。 |
| E09.ag041_postpaid_convergence | E09 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-45是、Q-46是、Q-44否 | 行为未变 | v4.4：记录验证付费后前缀/claim漂移或目标丢失收为唯一Uncertain、支付事实不丢（exploration_postpaid_v42.rs:1311–1583,1735–1964）；不涉开发证据资格或多候选，行为未改变。 v4.5：Q-45/Q-46只新增字段登记、runner合同核验与C_probe_compose启用事实；本记录未触及。 |
| E09.ag048_terminal_classes | E09 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-45是、Q-46是、Q-44否 | 相邻不矛盾 | v4.4：记录验证优化步终态使用封闭typed类与固定码、决策行为不变（step_terminal_classes_v42.rs:2337–2547）；类来源于fixture开发选择、与Q-29同语义区但未把执行结果当Skill证据，属相邻不矛盾。 v4.5：Q-45/Q-46只新增字段登记、runner合同核验与C_probe_compose启用事实；本记录未触及。 |
| E09.ag103_fairness_waits_legal_set_v42 | E09 | v4.4：Q-27否、Q-29是、Q-30是；v4.5：Q-45是、Q-46是、Q-44否 | 行为未变 | v4.4：记录验证core legal_candidates预选过滤与公平等待只对完成轮合法跳过机会（fairness_waits_legal_set_v42.rs:419–636）；Q-29/Q-30不涉，行为未改变。 v4.5：Q-45/Q-46只新增字段登记、runner合同核验与C_probe_compose启用事实；本记录未触及。 |
| E10.ag032_replay_redacted_reads | E10 | v4.4：Q-36否；v4.5：Q-54是、Q-36否 | 行为未变 | v4.4：记录验证回放各读点在清理中间态返回命名Conflict、墓碑kind精确比较（replay_redacted_reads_v42.rs:912–1233,1319–1519；#198将记录种类适配为current v3，命名Conflict语义不变）；E10适用的Q-36为“否”（v3已按首命中计数），行为未改变。 v4.5：记录验证回放脱敏读命名Conflict语义；Q-54的E10部分只登记过滤次序，本记录未触及。 |
| E13.ag022_consolidation_closure | E13 | v4.4：Q-27否；v4.5：Q-29是、Q-58否、Q-27否 | 相邻不矛盾 | v4.4：记录验证监测撤销闭包、pass/fail配对v2、持久提案与唯一staging（monitoring_consolidation_v42.rs:1430–1531,1723–1851,2386–2512,2766–2991）；E13适用的Q-27只对应HS03逐编辑标注“仅登记、只作假设”，不改变本记录行为。 v4.5：记录验证监测撤销闭包、pass/fail配对v2、持久提案与唯一staging（monitoring_consolidation_v42.rs:1430–1531,1723–1851,2386–2512,2766–2991）；配对与提案均来自Fixture出处报告且出处恒标ProgramFixture、伪标trusted_execution被拒，不以identity_only配对作Skill内容证据，与Q-58/Q-29相邻不矛盾，行为未改变。 |
| E13.ag030_consolidation_budget_stage | E13 | v4.4：Q-27否；v4.5：Q-29是、Q-58否、Q-27否 | 行为未变 | v4.4：记录验证巩固调用记入Consolidation预算阶段与双向阶段校验（consolidation_budget_stage_v42.rs:377–430,1568–1766,1869–2004）；E13适用的Q-27只对应HS03逐编辑标注“仅登记、只作假设”，不改变本记录行为。 v4.5：记录验证巩固调用记入Consolidation预算阶段与双向阶段校验（consolidation_budget_stage_v42.rs:377–430,1568–1766,1869–2004）；不涉配对证据资格，Q-29/Q-58未触及，行为未改变。 |
| E13.ag031_bounded_terminal_category | E13 | v4.4：Q-27否；v4.5：Q-29是、Q-58否、Q-27否 | 行为未变 | v4.4：记录验证128字节UTF-8安全终态类别与长理由收口（consolidation_terminal_category_v42.rs:133–257,1164–1336）；E13适用的Q-27只对应HS03逐编辑标注“仅登记、只作假设”，不改变本记录行为。 v4.5：记录验证128字节UTF-8安全终态类别与长理由收口（consolidation_terminal_category_v42.rs:133–257,1164–1336）；不涉配对证据资格，Q-29/Q-58未触及，行为未改变。 |
| E13.ag081_local_scoped_acceptance | E13 | v4.4：Q-27否；v4.5：Q-29是、Q-58否、Q-27否 | 相邻不矛盾 | v4.4：记录验证监测/巩固原报告与执行输入实际绑定（头注明言登记纯函数控制只证typed收据）（monitoring_evidence_binding_v42.rs:1475–1583,1793–1968,2032–2095）；E13适用的Q-27只对应HS03逐编辑标注“仅登记、只作假设”，不改变本记录行为。 v4.5：记录验证监测/巩固原报告与执行输入实际绑定（monitoring_evidence_binding_v42.rs:1475–1583,1793–1968,2032–2095）；其登记纯函数（identity_only性质）配对只得NoContrast/NoChange并只证收据与绑定链，提案出处为ProgramFixture，正是v4.5 §11.5允许identity_only证明的内容，相邻不矛盾，行为未改变。 |
| E16.1.ag076_local_scoped_acceptance | E16.1 | v4.4：Q-27否；v4.5：Q-43是 | 行为未变 | v4.4：记录验证有界内容驱动导入ProgramFixture的操作/匹配/闭包/编码（import_fixture_v42.rs:283–327,607–737,1024–1095）；HS07另计参数截断针对轨迹覆盖记录，本记录未涉，行为未改变。 v4.5：Q-43未触及。 |
| E16.1.ag070_local_scoped_acceptance | E16.1 | v4.4：Q-27否；v4.5：Q-43是 | 相邻不矛盾 | v4.4：记录验证有界本地开发物料读取器（import_development_material_v42.rs:326–405）及其字符截断计数与partial标记（748–749）；HS07只在条数/字符截断之外另计参数截断，与本记录相邻但不矛盾，行为未改变。 v4.5：所验字符截断计数与partial与Q-43同向，参数截断计数为新增义务，相邻不矛盾。 |
| E16.3.seed_install_reset_controller_acceptance | E16.3 | v4.4：Q-35否；v4.5：Q-55否、Q-59否 | 行为未变 | v4.4：记录验证种子B/L/U安装与reset staging不auto-Active（seeds_v41.rs:33–156,434–501；#168已把缺失/损坏B改为Unknown，L148–155）；E16.3适用的Q-35为“否”（现行已取Unknown），行为未改变。 v4.5：记录验证种子B/L/U安装与reset staging不auto-Active（seeds_v41.rs:33–156,434–501）；Q-55为否且本记录未把身份字段赋予U，Q-59为措辞登记，行为未改变。 |
| E16.3.ag127_v43_seed_classification | E16.3 | v4.4：Q-35否；v4.5：Q-55否、Q-59否 | 行为未变 | v4.4：记录验证纯B/L/U分类对损坏摘要或缺身份失败关闭为Unknown（seed_classification_v43.rs:21–167，其中L/U缺失或无效亦为Unknown见90–103）；与v4.4 Q-35一致且Q-35为“否”，行为未改变。 v4.5：记录验证纯B/L/U分类对损坏摘要或缺身份失败关闭为Unknown（seed_classification_v43.rs:21–167）；其身份字段只在B/L、U以内容摘要为身份，正与v4.5 Q-55表述一致（Q-55为否），行为未改变。 |
| E16.5.gate_functions_controller_acceptance | E16.5 | v4.4：Q-28否、Q-32是、Q-39否、Q-41是；v4.5：Q-54是、Q-56是、Q-27否、Q-59否 | 行为未变 | v4.4：记录验证容量/恢复闭包/部署安全纯函数门（头注V073仅为场景族列举，无许可断言）（capacity_v41.rs:137–284,286–486,544–794）；Q-32只改许可登记/README、Q-41只界定时钟下界的发生时间列，本记录均未断言，Q-28/Q-39为否，行为未改变。 v4.5：记录验证容量/恢复闭包/部署安全纯函数门（capacity_v41.rs:137–284,286–486,544–794）；未断言参与下界的列集合或installed_on不参与（M35无该下界），Q-54维持原结论；Q-56只涉README/许可措辞机检，本记录不涉，Q-27/Q-59仅登记，行为未改变。 |
| E16.5.ag014_capacity_gates_wired | E16.5 | v4.4：Q-28否、Q-32是、Q-39否、Q-41是；v4.5：Q-54是、Q-56是、Q-27否、Q-59否 | 行为未变 | v4.4：记录验证MVP容量门接入六个真实入口并跨命名空间计数（capacity_v41.rs:796–867）；Q-32只改许可登记/README、Q-41只界定时钟下界的发生时间列，本记录均未断言，Q-28/Q-39为否，行为未改变。 v4.5：记录验证MVP容量门接入六个真实入口并跨命名空间计数（capacity_v41.rs:796–867）；未断言参与下界的列集合或installed_on不参与（M35无该下界），Q-54维持原结论；Q-56只涉README/许可措辞机检，本记录不涉，Q-27/Q-59仅登记，行为未改变。 |
| E16.5.ag018_startup_recovery_gate | E16.5 | v4.4：Q-28否、Q-32是、Q-39否、Q-41是；v4.5：Q-54是、Q-56是、Q-27否、Q-59否 | 行为未变 | v4.4：记录验证启动恢复隔离门只读复核、独立锚与无覆盖准入回执（startup_gate_v42.rs:296–624,625–808,1354–1437）；Q-32只改许可登记/README、Q-41只界定时钟下界的发生时间列，本记录均未断言，Q-28/Q-39为否，行为未改变。 v4.5：记录验证启动恢复隔离门只读复核、独立锚与无覆盖准入回执（startup_gate_v42.rs:296–624,625–808,1354–1437）；未断言参与下界的列集合或installed_on不参与（M35无该下界），Q-54维持原结论；Q-56只涉README/许可措辞机检，本记录不涉，Q-27/Q-59仅登记，行为未改变。 |
| E16.5.ag029_restore_coverage | E16.5 | v4.4：Q-28否、Q-32是、Q-39否、Q-41是；v4.5：Q-54是、Q-56是、Q-27否、Q-59否 | 行为未变 | v4.4：记录验证恢复保护动作schema集合Rust/Python相等与锚前进隔离（restore_coverage_v42.rs:1024–1207,1385–1430,1510–1570）；Q-32只改许可登记/README、Q-41只界定时钟下界的发生时间列，本记录均未断言，Q-28/Q-39为否，行为未改变。 v4.5：记录验证恢复保护动作schema集合Rust/Python相等与锚前进隔离（restore_coverage_v42.rs:1024–1207,1385–1430,1510–1570）；未断言参与下界的列集合或installed_on不参与（M35无该下界），Q-54维持原结论；Q-56只涉README/许可措辞机检，本记录不涉，Q-27/Q-59仅登记，行为未改变。 |
| E16.5.ag051_economic_restore | E16.5 | v4.4：Q-28否、Q-32是、Q-39否、Q-41是；v4.5：Q-54是、Q-56是、Q-27否、Q-59否 | 行为未变 | v4.4：记录验证经济回放记录按精确schema纳入恢复与启动保护（#198改动未触及所引函数体）（restore_economic_v42.rs:1248–1346,1487–1622）；Q-32只改许可登记/README、Q-41只界定时钟下界的发生时间列，本记录均未断言，Q-28/Q-39为否，行为未改变。 v4.5：记录验证经济回放记录按精确schema纳入恢复与启动保护（restore_economic_v42.rs:1248–1346,1487–1622）；未断言参与下界的列集合或installed_on不参与（M35无该下界），Q-54维持原结论；Q-56只涉README/许可措辞机检，本记录不涉，Q-27/Q-59仅登记，行为未改变。 |
| E16.5.ag125_v43_single_writer | E16.5 | v4.4：Q-28否、Q-32是、Q-39否、Q-41是；v4.5：Q-54是、Q-56是、Q-27否、Q-59否 | 行为未变 | v4.4：记录验证本地OS单写者目录独占、生产入口门与活写者保留（single_writer_v43.rs:78–296）；Q-32只改许可登记/README、Q-41只界定时钟下界的发生时间列，本记录均未断言，Q-28/Q-39为否，行为未改变。 v4.5：记录验证本地OS单写者目录独占、生产入口门与活写者保留（single_writer_v43.rs:78–296）；未断言参与下界的列集合或installed_on不参与（M35无该下界），Q-54维持原结论；Q-56只涉README/许可措辞机检，本记录不涉，Q-27/Q-59仅登记，行为未改变。 |
| E16.5.ag129_v43_system_clock_conversion | E16.5 | v4.4：Q-28否、Q-32是、Q-39否、Q-41是；v4.5：Q-54是、Q-56是、Q-27否、Q-59否 | 相邻不矛盾 | v4.4：记录验证系统墙钟换算单位保持且纪元前样本命名拒绝（clock_unix_v43.rs:6–32）；Q-41只界定“本库最大发生时间”取哪些列，未改纪元前拒绝，属同条款相邻不矛盾。 v4.5：记录验证系统墙钟换算单位保持且纪元前样本命名拒绝（clock_unix_v43.rs:6–32）；其断言纪元前/首个时钟拒绝与Q-54新增的发生时间列条目校验相邻不矛盾（M35无数据库下界），维持原adjacent；Q-56只涉README/许可措辞机检，本记录不涉，Q-27/Q-59仅登记，行为未改变。 |
| E16.5.ag129_v43_broker_first_clock_refusal | E16.5 | v4.4：Q-28否、Q-32是、Q-39否、Q-41是；v4.5：Q-54是、Q-56是、Q-27否、Q-59否 | 相邻不矛盾 | v4.4：记录验证broker首个时钟拒绝先于预留与夹具派发（clock_fail_closed_v43.rs:31–133）；Q-41新增的发生时间列清单与类型校验未被本记录断言或否定，属相邻不矛盾。 v4.5：记录验证broker首个时钟拒绝先于预留与夹具派发（clock_fail_closed_v43.rs:31–133）；其断言纪元前/首个时钟拒绝与Q-54新增的发生时间列条目校验相邻不矛盾（M35无数据库下界），维持原adjacent；Q-56只涉README/许可措辞机检，本记录不涉，Q-27/Q-59仅登记，行为未改变。 |
| E16.6.ag086_local_scoped_acceptance | E16.6 | v4.4：Q-27否、Q-31否、Q-32是、Q-42否；v4.5：Q-50是、Q-49否、Q-60否 | 相邻不矛盾 | v4.4：记录验证部分证据登记表（expected_merge_for_pr，support_scope.rs:476–570，#132在L562）与历史记录保留原plan摘要（1607–1659）；Q-32许可状态不在其验证范围，Q-31/Q-42的入口切换与撤出治理由AG-182另行更新，行为未改变。 v4.5：记录验证部分证据登记与历史记录不改写（support_scope.rs:476–570,1606–1719）；Q-50/Q-42把复核标记、替代关系与撤出清单写成机器口径，与所验历史记录保留、required不计入的规则同向不矛盾，新增字段属E16.6本次重绑定的新增义务；Q-49/Q-60仅登记，行为未改变。 |
| E16.6.ag131_v43_project_license_metadata | E16.6 | v4.4：Q-27否、Q-31否、Q-32是、Q-42否；v4.5：Q-50是、Q-49否、Q-60否 | 行为被改变（撤出） | v4.4：记录验证项目许可manifest登记与Rust元数据守卫，但其登记的验证（源4232a226、摘录日志）断言许可状态为proprietary_unreleased；v4.4§2.8.1已把状态改为source_available_personal_noncommercial_free并替代旧状态，故属被改变的旧行为，须撤出并按M35现行测试重新登记新记录。 v4.5：v4.5增量项不改变本记录：其所验project_license登记不涉Q-50机器口径；但v4.4复核已判changed（记录源4232a226断言proprietary_unreleased，Q-32），综合仍为changed，按recheck-table既定指派处理。 |

计数：84 条 = 行为未变 54 + 相邻不矛盾 25 + 行为被改变 5；79 条标 rechecked_v4.5，5 条移入 withdrawn_records；并集外 14 条（E11 1、E12 3、E14 3、E16.2 3、E16.4 2、E17 1、E18 1）保留原 plan_recheck 值。侦察中的四项口径（只按“是”项判定、以记录当时验证内容定性、E09.ag033 changed 与 E09.ag039 adjacent、fixture 出处不算 identity_only）与 R2 四项裁决（E05.ag080 wire golden 判 changed、经 finalize/grade 间接调用 decide 记 adjacent、E06 两条与 E16.6.ag086 记 adjacent）按卡执行。

### 撤出记录归属表（待重验清单，24 条；§18.10 撤出记录的重验归属）

| 记录 id | 所属 E | 撤出 PR | 落点条款 | 重验方式 | 负责的修补任务 | 状态 |
|---|---|---|---|---|---|---|
| E10.controller_acceptance | E10 | #165 | §7.4.2–§7.4.3 | 以 rsia.replay.engine.v3 按当前版本直接重验（本 PR 单目标重跑，新记录 supersedes 旧 id） | AG-182 | superseded（→ E10.ag182_replay_v3_controller_acceptance） |
| E10.replay_management_controller_acceptance | E10 | #165 | §7.4.2–§7.4.3 | 以 rsia.replay.engine.v3 按当前版本直接重验（本 PR 单目标重跑，新记录 supersedes 旧 id） | AG-182 | superseded（→ E10.ag182_replay_v3_management_controller_acceptance） |
| E10.ag064_replay_episode_recovery | E10 | #165 | §7.4.2–§7.4.3 | 以 rsia.replay.engine.v3 按当前版本直接重验（本 PR 单目标重跑，新记录 supersedes 旧 id） | AG-182 | superseded（→ E10.ag182_replay_v3_episode_recovery） |
| E10.ag103_replay_fairness_waits_legal_set_v42 | E10 | #165 | §7.4.2–§7.4.3 | 以 rsia.replay.engine.v3 按当前版本直接重验（本 PR 单目标重跑，新记录 supersedes 旧 id） | AG-182 | superseded（→ E10.ag182_replay_v3_fairness_waits_legal_set） |
| E08.ag037_economic_replay_cleanup | E08 | #165 | §7.4.2–§7.4.3 | 以 rsia.replay.engine.v3 按当前版本直接重验（本 PR 单目标重跑，新记录 supersedes 旧 id） | AG-182 | superseded（→ E08.ag182_replay_v3_economic_cleanup） |
| E09.ag074_local_scoped_acceptance | E09 | #165 | §7.4.2–§7.4.3 | 以 rsia.replay.engine.v3 按当前版本直接重验（本 PR 单目标重跑，新记录 supersedes 旧 id） | AG-182 | superseded（→ E09.ag182_replay_v3_legal_prefix） |
| E16.6.ag012_plan_v42_index_binding | E16.6 | #165 | §18.9 | 已被 v4.3 索引绑定替代：只登记替代关系，不重验、不复活；当前版本绑定见 E16.6.ag182_plan_v45_index_binding | AG-124 | replaced_by_binding |
| E04.ag097_broker_close_before_refuse | E04 | #165 | §11.3.1/§11.6 | 随统一撤销判定的修补任务，按 §11.3.1 执行关闭写集重验（Q-51） | AG-119 | pending_recheck |
| E04.ag105_broker_bound_recovery_close_v42 | E04 | #165 | §11.3.1/§11.6 | 随统一撤销判定的修补任务，按 §11.3.1 执行关闭写集重验（Q-51） | AG-119 | pending_recheck |
| E08.ag046_write_side_revocation | E08 | #165 | §11.3.1 | 随统一撤销判定的修补任务，按 §11.3.1 统一撤销判定（精确 kind、首写前零写入）重验（Q-50） | AG-119 | pending_recheck |
| E08.ag059_manifest_prewrite_gate | E08 | #165 | §11.3.1 | 随统一撤销判定的修补任务，按 §11.3.1 统一撤销判定（精确 kind、首写前零写入）重验（Q-50） | AG-119 | pending_recheck |
| E04.ag054_typed_predispatch | E04 | #169 | §7.3.1 | 随时钟下界的修补任务重验（含 v4.4 发生时间列） | AG-132 | pending_recheck |
| E06.controller_acceptance | E06 | #165 | §5.5.2 | 随 Tool-only 映射与能力上限的修补任务重验 | AG-169 | pending_recheck |
| E13.controller_acceptance | E13 | #165 | §5.5.2 | 随 Tool-only 映射与能力上限的修补任务重验 | AG-169 | pending_recheck |
| E16.4.real_store_rejection_controller_acceptance | E16.4 | #165 | §5.5.2 | 随 Tool-only 映射与能力上限的修补任务重验 | AG-169 | pending_recheck |
| E09.ag042_recovery_counting | E09 | #165 | §7.2.3 | 随可修复失败来源与重置收据的修补任务重验 | AG-171 | pending_recheck |
| E13.ag028_consolidation_trigger | E13 | #165 | §6.1.2 | 随巩固管理入口的修补任务重验 | AG-139 | pending_recheck |
| E16.1.durable_import_controller_acceptance | E16.1 | #165 | §11.2.1 | 随原生 Trace 导出的修补任务重验 | AG-148 | pending_recheck |
| E02.ag062_required_improver_baselines | E02 | #165 | §5.1.2 | 随摘要字段可重算的修补任务重验 | AG-135b | pending_recheck |
| E09.ag033_run_next_trusted_gate | E09 | #200 | §6.7.4 | v4.4 Q-29：identity_only 纯函数执行结果被固定为 Trusted 节点质量与候选资格；随 candidate_consumption 合同按新记录重验 | AG-187 | pending_recheck |
| E16.6.ag131_v43_project_license_metadata | E16.6 | #200 | §2.8.1 | v4.4 Q-32：所登记的验证断言 proprietary_unreleased；本 PR 在 head 上重跑同名目标并以新记录 supersedes 替代 | AG-182 | superseded（→ E16.6.ag182_v45_project_license_metadata） |
| E05.ag052_versioned_batch_verdicts | E05 | #200 | §3.4 | v4.5 Q-57：断言 decide() 以 LCB<0 判 regressed（含 wire golden 冻结该分支输出）；修补任务纠正判据后按新记录 supersedes 重验 | AG-192 | pending_recheck |
| E05.ag080_local_scoped_acceptance | E05 | #200 | §3.4 | v4.5 Q-57：断言 decide() 以 LCB<0 判 regressed（含 wire golden 冻结该分支输出）；修补任务纠正判据后按新记录 supersedes 重验 | AG-192 | pending_recheck |
| E05.ag101_streaming_verdicts_v42 | E05 | #200 | §3.4 | v4.5 Q-57：断言 decide() 以 LCB<0 判 regressed（含 wire golden 冻结该分支输出）；修补任务纠正判据后按新记录 supersedes 重验 | AG-192 | pending_recheck |

19 条（#165 撤出 18 条 + #169 AG128-R2 更正撤出 E04.ag054）按真源 §18.10 表登记；本 PR 新撤出 5 条（E09.ag033、E16.6.ag131、E05.ag052、E05.ag080、E05.ag101）。卡验收条件 6 写“21 行”是 R2 增量侦察并入三条 E05 之前的数字，按真源与卡验收条件 5 登记 24 行。三条 E05 撤出记录由 AG-192 重验，本卡不重验、不复活。

### 缺失清单（reports/evidence-missing.json：42 路径 / 25 条记录）

按 §18.10：缺失路径不撤出已验状态、不改写路径与摘要、不重造文件；逐条登记路径、记录 id、字段、预期字节与 SHA-256、消失窗口与分组；预期身份不可得者登记 unknown 并写原因，重现一律失败。分组：
- `D1_old_machine_v41_controller_original`：28 路径 / 11 条记录（input_ref 11、log_path 11、record_source 6）。
- `D2_task_receipt_lost_between_20261014_and_20261023`：13 路径 / 13 条记录（log_path 13）。
- `D3_ag083_scratch_raw_log_not_located`：1 路径 / 1 条记录（log_path 1）。
- D1 的 log_path/record_source 身份为 unknown（索引从未钉其摘要；原件只在旧机，2026-09-30 交接即已登记），D1 的 input_ref 以记录自身 input_digest 为预期 SHA-256、字节未登记；D2 的 13 份 Task 回执以既有 input_ref 中钉住的字节与 SHA-256 为预期身份，消失窗口为 2026-10-14T11:54:14Z（最后一次主检出检查器仍见到）至 2026-10-23T14:35Z（索引审计已缺）；D3（AG-083 scratch 原始日志）预期身份取自 ctrl-input-ag-083 回执，2026-10-12T16:15:59Z 前已缺。分类与检索过程见本地 qa/claude-r8-prep/missing-evidence/（不上传）。
- 25 条受影响记录按其登记命令在本 PR head 上单目标重跑，以新记录 `completes`=[旧 id] 登记（见下文“重跑与重验”）；不冒充旧证据。

### §16.8 固定来源读取范围与授权外研究动作（§18.10 动作合同）

- HSF01–HSF16、HSP01 的实际读取：由 Harness² 审计代理在本地快照与固定提交 1cfe254（tree edc756a4）的原始克隆上读取，用于事实核验与借鉴矩阵裁决；16 个 blob 由起草代理在本地快照上现算与 §16.8 一致，tree 由 W0 经 GitHub API 核对（2026-10-23 读取，commit date 2026-10-14T02:22:49Z）。本轮未按行段单独记录各文件读取范围，登记为“快照全文/定位读取，未分行段”，逐文件行段登记 not_executed（§18.11 动作合同）；直接复制或改编前须按 §2.8 重新逐文件登记行段与许可。HSP01 为 arXiv:2610.14224v1 Section 2.2.1 与 Appendix F（审计代理 2026-10-23 核对）。派生索引 hsf_sources 的 read_range 统一为 `ledger`，即指向本条登记。
- 授权外研究动作（§2.8 v4.4 补充；V123.c v4.5 补充）：GPT 在本地研究目录以子进程执行上游 harness2/demo.py（2 个任务、28 次本地执行、32 次脚本化编辑器调用、无模型调用），属 §2.8 第一段授权之外；其结果不作任何依据（含“经验库被读取”的说法——实际编辑器读库 0 次），不出现在派生索引记录、验证记录、研究计划参数或效果报告的任何字段中。

### 派生索引说明槽更正

- E04 remaining[0] 原写“no actual provider transport … is wired”，与同条目已由 #165 更正的 status_reason（formal_local_host 已接外部 provider transport）矛盾；改为“provider transport 已接线、合格隔离 runner 未接线、无真实 provider 调用已验证”。
- E16.5 remaining[2]、E16 remaining[1] 与 overall_remaining 末项原写“round 2 内部对象容量须先修订 §3.3.1”，但 v4.3 §3.3.2（Q-01）已登记这些初值；改为“初值已由 §3.3.2 登记，真实入口的执行仍未验证”。E16 status_reason 的“bound to v4.2 (AG-012)”改为 v4.5（AG-182），E16.6 status_reason 改写为 v4.5 口径并追加 remaining 一条；E16.6 implementation_files 加入 reports/evidence-missing.json。
- 其中 E16 remaining、overall_remaining 与 E16 status_reason 三处更正超出卡列（卡只列 E04/E16.5），经主控确认保留（返修单 R8-1 R-15）。返修提交另把 E16.6 status_reason 的补登口径改为“v4.3 首验绑定 + rechecked_v4.5”并写入两套检查器的钉扎计数，E16.6 remaining 末项补“默认模式登记路径为 registered, not checked，Rust 无证据库输入（AG-200）”，V116 remaining 去掉已撤出 AG131 记录的引用（R-13），绑定记录 E16.6.ag182_plan_v45_index_binding 的 scope 注明“运行于 a9ad4587 时的索引状态；最终索引由主控在最终 head 上的十一门覆盖”（R-16）。

### 索引补登（backfill.json 可进索引 10 条 + 有条件 2 条）

均为既有首验全量运行中的单目标结果（不是新执行）。按主控裁决（审查 R-04 方案 ii）保留首验时的 plan_version v4.3 / `73dd00bf5ef6e9757d0c391c9a8be4ca92b6e06a750aacb542817f82cbd208a7`，并以 `plan_recheck: rechecked_v4.5` 登记已按 v4.4/v4.5 复核（12 条所属 E 均在并集内，检查器强制）；复核依据见表末列，口径同上文复核表（只按适用的“改变实现行为=是”项判定；源码行号为各记录 source_sha 上的文件）。command 为声明式 8 词单目标；execution.command 为产生该 .log.test 的字面命令，出处（审查 R-11）：12 份首验均由 `out/audit-20260930/qa/codex-r7-ctrl_verify.sh`（SHA-256 `982635c6603095a7f47f224a5f285ef89b534ac202c18a390c5d703f41528cad`）执行，其第 13 行为 test 门 `cargom test --workspace --locked --no-fail-fast > "$LOG.test"`；该行与 `out/audit-20260930/qa/ctrl_verify.sh`（SHA-256 `9c8776489d4c7e8b6174ed6557a161d846a03c184e9821c615839f5b441ec73c`）第 13 行逐字相同，#178 回执另把 ctrl_verify.sh 列为绑定源。多目标 PR 只登记一个主目标，其余目标与 lib 单测只登台账。

| PR | 任务 | 记录 id | E | 单目标（计数） | 首验日志 | 说明 | v4.4/v4.5 复核依据（适用是项 → 结论） |
|---|---|---|---|---|---|---|---|
| #178 | AG-150 | E16.5.ag150_readonly_dependency_api_v43 | E16.5 | readonly_dependency_api_v43（1 过 / 0 败） | `out/audit-20260930/qa/codex-r7-ag150/controller-9ab6d39-M13-r1.log.test` | — | 适用是项 v4.4 Q-32/Q-41、v4.5 Q-54/Q-56；readonly_dependency_api_v43 只验只读 SQLite 依赖 API 的无检查点配置往返（同文件 L8），不断言许可状态/README（Q-32/Q-56），也不断言时钟下界的列集合或 installed_on（Q-41/Q-54）→ 行为未变 |
| #180 | AG-136 | E02.ag136_management_registry_single_source_v43 | E02 | management_registry_v43（6 过 / 0 败） | `out/audit-20260930/qa/external-ag136-r7/independent-accept-eight-gates-m15-r1.log.test` | 有条件：engine 目标 management_registry_boundary_v43 断言已登记未接线名返回 unknown（Q-38/Q-53 已改变），不入本记录，待 AG-186 补全 | 适用是项 v4.4 Q-38/Q-40、v4.5 Q-52/Q-53；management_registry_v43 只验 73 名单源注册表快照、族与平面计数、审阅投影、付费策略与表外名不认（同文件 L746–907），不断言已登记名的 not_wired/unknown 返回或作业记录版本，快照与 Q-53 的平面/已接线判定相邻（后续登记属新增义务）→ 相邻不矛盾；engine 目标 management_registry_boundary_v43 L150 断言已登记未实现名返回 unknown（Q-38/Q-53 已改变），已按 note 剔除，待 AG-186 补全 |
| #187 | AG-137（F02） | E05.ag137_evaluator_control_authentication | E05 | formal_external_host_authority_v42（9 过 / 0 败） | `out/audit-v43-remediation-20261016/F02-head-r5.full.log.test` | lib 单测模块（formal_authority::…）不合 `--lib …::tests` 语法，只登台账 | 适用是项 v4.4 Q-34、v4.5 Q-57/Q-47；formal_external_host_authority_v42 验 Evaluator 控制注册认证与已认证请求见证，finalize 只走正效应 decide()（同文件 L1669 断言 Verdict::Improved），未断言 LCB<0⇒regressed，不涉可行性规划/门 → 与 Q-57 相邻不矛盾 |
| #188 | F01 | E06.f01_builtin_generation_strategy_authority | E06 | compiler_formal_binding_v42（5 过 / 0 败） | `out/audit-v43-remediation-20261016/F01-head-r1.full.log.test` | lib 单测模块（formal_authority::…）不合 `--lib …::tests` 语法，只登台账 | 适用是项 v4.5 Q-38（E06 不在 v4.4 并集）；compiler_formal_binding_v42 只验编译边界的内置策略权威（同文件 L10–250：重放绑定、旧选择拒绝、生产编译拒绝自定义搜索与非内置基线），不涉管理操作未接线固定码或平面错配 → 行为未变 |
| #189 | AG-143（F06） | E04.ag143_external_sampling_projection_v43 | E04 | external_model_sampling_projection_v43（3 过 / 0 败） | `out/audit-v43-remediation-20261016/F06-head-r1.full.log.test` | lib 单测模块（formal_authority::…）不合 `--lib …::tests` 语法，只登台账 | 适用是项 v4.4 Q-40/Q-41、v4.5 Q-52/Q-54；external_model_sampling_projection_v43 只验本地宿主的确定性采样投影与未绑定发送拒绝（同文件 L266–281 三例），不涉作业记录版本/v2，也不断言时钟下界列集合 → 行为未变 |
| #191 | AG-134 | E04.ag134_root_budget_cancellation_v43 | E04 | root_budget_cancellation_v43（8 过 / 0 败） | `out/audit-20260930/qa/codex-r7-ag134/controller-dae0d8e-M26-r1.log.test` | — | 适用是项 v4.4 Q-40/Q-41、v4.5 Q-52/Q-54；root_budget_cancellation_v43 只验 in-memory RootBudget 各阶段取消记账（同文件 L45–119、L126、L153：预留释放、已派发不退款、超时不确定、已结算拒绝、错令牌/过期租约不改字段、租约边界处的分阶段取消合同、已取消/不确定预算不可重开或释放未知用量；L126、L153 两例由 AG-200 按审查 S-3 补登，结论不变），不是管理作业记录（Q-40/Q-52），不涉时钟下界（Q-41/Q-54）→ 行为未变 |
| #192 | AG-153 | E16.5.ag153_shared_prepare_contract_v43 | E16.5 | prepare_contract_v43（12 过 / 0 败） | `out/audit-20260930/qa/codex-r7-ag153/controller-a76d734-M27-r2.log.test` | — | 适用是项 v4.4 Q-32/Q-41、v4.5 Q-54/Q-56；prepare_contract_v43 只验 HostPrepareConfig/PrepareOperation 共用合同的字节、指纹与原拒绝类（同文件 L56–335），不断言时钟下界列集合或 installed_on，也不涉许可/README → 行为未变。归 E16.5 的理由（审查 R-15）：AG-153 卡文“仅 E16.5/V110.c 的共用类型前置，不是已实现时钟门或 V110 通过” |
| #194 | AG-114 | E16.6.ag114_evidence_identity_ownership | E16.6 | support_scope（33 过 / 0 败） | `out/audit-20260930/qa/codex-r7-ag114/newcomplete-controller-17579a3-M29-r1.log.test` | — | 适用是项 v4.4 Q-32、v4.5 Q-50；support_scope 验证据 id 命名空间、owner 与 test_entry 归属，其许可断言已是新状态 source_available_personal_noncommercial_free / `164d4296…`（与 Q-32 同向）；v4.3 常量与缺 plan_recheck 机器口径属该 head 的历史绑定，Q-50 机器口径由本 PR 另行登记 → 相邻不矛盾 |
| #195 | AG-144 | E01.ag144_feasibility_approval_alignment_v43 | E01 | statistical_feasibility_v43（21 过 / 0 败） | `out/audit-20260930/qa/codex-r7-ag144/ctrl-0be75c2-M30-r1-runtime/eight/full-eight.log.test` | 有条件：非劣等号用例（Q-34/Q-47 已改变）不入本记录，待 AG-185 补全 | 适用是项 v4.4 Q-30/Q-34、v4.5 Q-45/Q-46/Q-47/Q-57；statistical_feasibility_v43 验可行性规划与批准门对齐：非劣等号用例（同文件 L236 NoninferiorityEqualityPending）正是 Q-34/Q-47 改变的旧行为，已按 note 剔除，待 AG-185 补全；L845/869/903 的 RegressedSequential 是 SequentialRejectOnly 的序贯早停而非 decide() 固定样本 LCB 分支，Q-57 未触及；不涉 C 子条件与候选生成前登记（Q-30/Q-45），Q-46 只新增默认关闭方法的启用/停用事实，未触及 → 相邻不矛盾 |
| #197 | AG-120 | E05.ag120_one_execution_receipt_per_call | E05 | late_grader_after_final_report_v42（29 过 / 0 败） | `out/audit-20260930/qa/codex-r7-ag120/AG120-controller-M32-r1-runtime/full-eight.log.test` | — | 适用是项 v4.4 Q-34、v4.5 Q-57/Q-47；late_grader_after_final_report_v42 验一次调用一份执行收据，finalize 只走正效应 decide()（同文件 L966/970 断言 Verdict::Improved），未断言退化分支，不涉可行性 → 与 Q-57 相邻不矛盾 |
| #198 | AG-147 | E10.ag147_replay_lineage_versioning_v43 | E10 | replay_lineage_v43（7 过 / 0 败） | `out/audit-20260930/qa/codex-r7-ag147/AG147-M33-controller-79f1c7b-r1-runtime/full-eight.log.test` | 六条回放撤出记录另由本 PR 单目标重验，不以 #198 首验替代 | 适用是项 v4.5 Q-54（v4.4 Q-36 为否）；replay_lineage_v43 只验版本化回放谱系摘要与静态旧读（同文件 L62–249），Q-54 的 E10 部分只登记过滤次序，本记录未触及 → 行为未变 |
| #199 | AG-122 | E05.ag122_ga18_bound_cycle_validation | E05 | formal_external_host_authority_v42（16 过 / 0 败） | `out/audit-20260930/qa/codex-r7-ag122/AG122-M34-independent-22609c6-r3-runtime/full-eight.log.test` | — | 适用是项 v4.4 Q-34、v4.5 Q-57/Q-47（Q-37/Q-48 为否，GA18 附着环正是其内容）；formal_external_host_authority_v42 验 GA18 附着环对照绑定执行事实，finalize 只走正效应 decide()（同文件 L1690 断言 Verdict::Improved），未断言退化分支，不涉可行性 → 与 Q-57 相邻不矛盾 |

只登台账 17 条：#165、#169、#172、#176（台账/索引 PR，其自身检查器子范围由后续绑定替代）、#171、#175、#177、#184、#196（无 Rust 单目标或只改 CI/依赖登记/Python）、#179、#181、#182、#183、#185、#186（README，#181/#185 的许可状态由 E16.6.ag182_v45_project_license_metadata 承载）、#190（台账 PR）、#193（只有 `dispatch::late_cancel_tests` 经 #[path] 引入的 lib 单测，现行语法不可声明；其 `:cancel_after_side_effect` 后缀属 Q-40/Q-52 过渡条款）。检查器 `--lib` 语法本卡不扩展。

### 第七阶段 PR 台账补登（#169、#176–#199，共 25 节）

每节只登记合并事实与首验材料位置；状态最高 implemented_not_verified + 首次技术验收（README/文档 PR 无运行时验收）；GPT 二验状态按 final-pr-matrix 原文登记，不是 Claude 二次验收。所有节通用的不可声明项：整 E/V、真实 provider/宿主/沙箱、付费实验、历史 T001–T126 与 E14 冻结合同、效果、收益、发行、Claude 二次验收。

#### #169 AG-128 — AG-128 record v4.3 scoped deliveries and corrected history
- 合并：merged `58c013572c806dc464a248331aeca41bcc4226c3`，merged_at 2026-10-13T07:53:55Z；验收 head `4bf224198eb74e0499b068c96a11170a4ad700d4`。
- 首验材料：`out/audit-20260930/qa/inputs/codex-input-ag-128-4bf22419.json`；`out/audit-20260930/qa/codex-r7-ag128/accept-final-r2-4bf22419.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:149,160。
- GPT 二验（final-pr-matrix）：original_scope_pass_new_historical_ownership_counterexample_failed_closed_by_PR194_and_M35。
- 索引：只登台账（台账/索引PR（登记#165–#168合并与v4.3交付）；GPT二验新增历史归属反例在其head上失败（exit 101），已由#194/M35关闭——台账须如实登记该失败与关闭）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #176 AG-149 — AG-149 record scoped first acceptance for PR173-175 [skip ci]
- 合并：merged `5958b21096da4d4bfd9c8c602b7a82f57bddde77`，merged_at 2026-10-14T15:24:21Z；验收 head `ca04856b4936200ee077a57ef9e6388f6d8e5a9c`。
- 首验材料：`out/audit-20260930/qa/inputs/codex-input-ag-149-ca04856b-pr176.json`；`out/audit-20260930/qa/codex-r7-ag149/accept-ca04856b-pr176-r1.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:404,410。
- GPT 二验（final-pr-matrix）：original_scope_pass_new_historical_ownership_counterexample_failed_closed_by_PR194_and_M35。
- 索引：只登台账（台账/索引PR（登记#173–#175首验）；GPT二验新增归属反例在其head上失败（exit 101），已由#194/M35关闭）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #177 AG-151 — AG-151 register exact legacy license expressions [skip ci]
- 合并：merged `c0110181f0f83846dc14609613cb734300d04fe4`，merged_at 2026-10-15T00:13:59Z；验收 head `0c21916fdd1da21a66344c82c1a3082e524a5b53`。
- 首验材料：`out/audit-20260930/qa/inputs/codex-input-ag-151-0c21916-pr177.json`；`out/audit-20260930/qa/codex-r7-ag151/controller-0c21916-M12-r1.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:418,430。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：只登台账（无Rust单目标测试（依赖登记JSON与Python检查器））。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #178 AG-150 — AG-150: register safe SQLite reader dependency [skip ci]
- 合并：merged `8f38f4e39cceb67976f9120dc3f76fecce232c5c`，merged_at 2026-10-15T04:12:20Z；验收 head `9ab6d39dedebf481fae507269dbdc6672d9ce3e9`。
- 首验材料：`out/audit-20260930/qa/inputs/codex-input-ag-150-9ab6d39-pr178.json`；`out/audit-20260930/qa/codex-r7-ag150/controller-9ab6d39-M13-r1.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:528,536。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：本 PR 补登 `E16.5.ag150_readonly_dependency_api_v43`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #179 README — Update README progress to the v4.3 scoped baseline
- 合并：merged `6773dc4180dc4a6668e593d2823d808350215293`，merged_at 2026-10-15T08:41:42Z；验收 head `779cf2ff75f293f662767aeb0e81ba072e91e486`。
- 首验材料：out/audit-20260930/codex-r7-acceptance.md:726（仅进度提及）。
- GPT 二验（final-pr-matrix）：documentation_scope_reviewed_runtime_not_applicable。
- 索引：只登台账（纯文档PR；GPT二验 documentation_scope_reviewed_runtime_not_applicable）。
- 状态：文档范围已审，运行时不适用；不可声明项见本小节开头。

#### #180 AG-136 — AG-136: single-source management registry and real control.review route [skip ci]
- 合并：merged `daa618a6a78cdb815102d22c57026d16e9d7aa90`，merged_at 2026-10-15T11:56:15Z；验收 head `b16ea35367afd73d14061f8f85d905c44371b137`。
- 首验材料：`out/audit-20260930/qa/external-ag136-r7/independent-acceptance-m15-r1.json`；`out/audit-20260930/qa/external-ag136-r7/independent-accept-eight-gates-m15-r1.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:915。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：本 PR 补登 `E02.ag136_management_registry_single_source_v43`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #181 README — README intro refresh and source-available personal license [skip ci]
- 合并：merged `d576d98649fddc589b286712a15dc6655b5c970d`，merged_at 2026-10-15T14:17:56Z；验收 head `3c877b7f83440e36b8dda7e1530cd15e92c862a4`。
- 首验材料：out/audit-20260930/codex-r7-acceptance.md:1062（仅提及合并）。
- GPT 二验（final-pr-matrix）：license_validator_scope_pass_no_separate_original_CPP_material_D1_open。
- 索引：只登台账（无Codex首验原件（GPT二验仅2项过滤运行，head exit 0）；其support_scope许可状态改动应由替代E16.6.ag131的新记录承载（在AG-182验收head重跑8词命令），不单列本PR记录）。
- 状态：README 与项目许可校验器（support_scope.rs 许可状态与 LICENSE 守卫）改动，implemented_not_verified；许可子范围由 E16.6.ag182_v45_project_license_metadata 重跑承载，README 徽章与措辞（Q-32/Q-56）由 AG-180 承接；不可声明项见本小节开头。

#### #182 README — README research-basis citations: link arXiv ids and pin commits [skip ci]
- 合并：merged `564b40d5c1e1b586835d025149c063820fd6f38f`，merged_at 2026-10-15T14:23:10Z；验收 head `4ea90dc8634c1442a6598a6a0d381d1f7045ea19`。
- 首验材料：无独立首验原件（文档 PR）。
- GPT 二验（final-pr-matrix）：documentation_scope_reviewed_runtime_not_applicable。
- 索引：只登台账（纯文档PR）。
- 状态：文档范围已审，运行时不适用；不可声明项见本小节开头。

#### #183 README — README: paper institutions and visible B0-L5 ladder [skip ci]
- 合并：merged `f3b7bc683915600615f118e3ceb5549df496d00d`，merged_at 2026-10-15T14:48:08Z；验收 head `90510186782b622ee06af9e8e2e017d742bbf0cc`。
- 首验材料：out/audit-20260930/codex-r7-acceptance.md:1082–1085（仅提及合并）。
- GPT 二验（final-pr-matrix）：documentation_scope_reviewed_runtime_not_applicable。
- 索引：只登台账（纯文档PR）。
- 状态：文档范围已审，运行时不适用；不可声明项见本小节开头。

#### #184 AG-152 — AG-152: enable governed rustix process feature [skip ci]
- 合并：merged `658b50d94ad1f73f907e205d190f927d00ca3ab9`，merged_at 2026-10-15T20:08:43Z；验收 head `0604629ca771bcb6a067a229ed48b755928443b5`。
- 首验材料：`out/audit-20260930/qa/codex-r7-ag152/Root-first-acceptance-H0604629-M19-r1/Root-first-acceptance-receipt-H0604629-M19-r1.json`；`out/audit-20260930/qa/codex-r7-ag152/controller-0604629-M19-r1.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:1399,1406。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：只登台账（只改evo-storage Cargo feature与依赖登记，无新增/修改Rust测试目标）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #185 README — README: bilingual Hy-Memory highlights, centered header/badges, and friendlier license [skip ci]
- 合并：merged `51a5f59c2048d3c146d534d9c81420a45fd6115c`，merged_at 2026-10-16T19:11:03Z；验收 head `deb64551872ca827bcc7d2b3f3452f43c2cc0bd2`。
- 首验材料：无独立 Codex 首验原件（GPT 二验做了许可校验器两组过滤运行）。
- GPT 二验（final-pr-matrix）：license_validator_scope_pass_no_separate_original_CPP_material_D1_open。
- 索引：只登台账（同#181：无Codex首验原件；LICENSE终版字节（SHA 164d4296…）的Rust守卫由替代E16.6.ag131的新记录承载）。
- 状态：README 与项目许可校验器（support_scope.rs 许可状态与 LICENSE 守卫）改动，implemented_not_verified；许可子范围由 E16.6.ag182_v45_project_license_metadata 重跑承载，README 徽章与措辞（Q-32/Q-56）由 AG-180 承接；不可声明项见本小节开头。

#### #186 README — README: add Antigravity to multi-host client examples [skip ci]
- 合并：merged `951124ea70290240b51a5e4cbd7841869884e5fa`，merged_at 2026-10-16T19:24:47Z；验收 head `d57830189c9690d7927d535cdbd95796e42fe7c5`。
- 首验材料：无独立首验原件（文档 PR）。
- GPT 二验（final-pr-matrix）：documentation_scope_reviewed_runtime_not_applicable。
- 索引：只登台账（纯文档PR）。
- 状态：文档范围已审，运行时不适用；不可声明项见本小节开头。

#### #187 AG-137 — fix: authenticate control registration as Evaluator [AG137/F02] [skip ci]
- 合并：merged `b166053d8fcf0fd8d046d3274e132a883b95e0bd`，merged_at 2026-10-17T06:25:46Z；验收 head `0cb69f9873d6520b49dc27a79ff03a571dd05052`。
- 首验材料：`out/audit-v43-remediation-20261016/delivery-receipt.json`；`out/audit-v43-remediation-20261016/F02-acceptance.json`；`out/audit-v43-remediation-20261016/F02-head-r5.full.log.test`（test 门退出 0）；out/audit-v43-remediation-20261016/delivery-receipt.json:repairs[0]。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：本 PR 补登 `E05.ag137_evaluator_control_authentication`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #188 F01 (collaboration) — fix: enforce builtin generation strategy authority [skip ci]
- 合并：merged `85612648c313a032ea369b56b62e593203e1abff`，merged_at 2026-10-17T08:14:29Z；验收 head `922af286abe641a96c24bea889537929f91922c3`。
- 首验材料：`out/audit-v43-remediation-20261016/delivery-receipt.json`；`out/audit-v43-remediation-20261016/F01-acceptance.json`；`out/audit-v43-remediation-20261016/F01-head-r1.full.log.test`（test 门退出 0）；out/audit-v43-remediation-20261016/delivery-receipt.json:repairs[1]。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：本 PR 补登 `E06.f01_builtin_generation_strategy_authority`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #189 F06 (collaboration) — fix: bind chat sampling projection and reject unbound sends [skip ci]
- 合并：merged `86221ff7a9ecf678d86f4c733befd4595cffd55e`，merged_at 2026-10-17T08:43:13Z；验收 head `8c19961c412a09192ba93a150bf9ec1a3080b699`。
- 首验材料：`out/audit-v43-remediation-20261016/delivery-receipt.json`；`out/audit-v43-remediation-20261016/F06-acceptance.json`；`out/audit-v43-remediation-20261016/F06-head-r1.full.log.test`（test 门退出 0）；out/audit-v43-remediation-20261016/delivery-receipt.json:repairs[2]。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：本 PR 补登 `E04.ag143_external_sampling_projection_v43`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #190 AG-111 — AG-111: preserve historical ledger facts [skip ci]
- 合并：merged `a8cbd08ac546b95f4ac3add51de464cae2af2bbe`，merged_at 2026-10-17T18:50:12Z；验收 head `ae49a0a045f0d5e6887cf765c5eee0339b25bb39`。
- 首验材料：`out/audit-20260930/qa/codex-r7-ag111/Root-AG111-PR190-fixed-head-first-acceptance-r1.json`；`out/audit-20260930/qa/codex-r7-ag111/controller-ae49a0a-M25-r1.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:1671,1680。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：只登台账（台账PR（P1/P2/P3历史事实守恒），无Rust测试）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #191 AG-134 — fix(engine): preserve root budget cancellation accounting [skip ci]
- 合并：merged `fc481354c047d63f90b687f1fef77b6f38ff9471`，merged_at 2026-10-18T02:26:55Z；验收 head `dae0d8e0da878a73991cb952bdd394f8d21b7ed5`。
- 首验材料：`out/audit-20260930/qa/codex-r7-ag134/controller-dae0d8e-M26-r1/Root-PR191-first-acceptance-r1.json`；`out/audit-20260930/qa/codex-r7-ag134/controller-dae0d8e-M26-r1.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:1759。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：本 PR 补登 `E04.ag134_root_budget_cancellation_v43`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #192 AG-153 — AG-153: share prepare contract across core and engine
- 合并：merged `1f624445d66caa8a854b9fa78abff4264439f505`，merged_at 2026-10-18T10:07:55Z；验收 head `a76d7341167b659ba2b996d9df9517b799f3005d`。
- 首验材料：`out/audit-20260930/qa/codex-r7-ag153/Root-PR192-first-acceptance-r1.json`；`out/audit-20260930/qa/codex-r7-ag153/controller-a76d734-M27-r2.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:1872,1877。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：本 PR 补登 `E16.5.ag153_shared_prepare_contract_v43`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #193 AG-154 — fix(engine): preserve committed results on late cancellation
- 合并：merged `6f13f82dd55243d8f33ae9152c85bcc09aaeaacd`，merged_at 2026-10-18T16:35:54Z；验收 head `2eeab6a41a75ccb11d93f0b853b512fc9eed764e`。
- 首验材料：`out/audit-20260930/qa/codex-r7-ag154/AG154-2eeab6a-M28-actual-independent-controller-native-and-raw-proof-r3.json`；`out/audit-20260930/qa/codex-r7-ag154/controller-2eeab6a-M28-r3.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:1896。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：只登台账（仅有src内cfg(test)模块dispatch::late_cancel_tests（经#[path]引入，crates/evo-engine/src/dispatch.rs:2128–2130），模块路径不以`::tests`结尾且不映射src/dispatch/late_cancel_tests.rs，现行8词语法不可声明→只能进台账；另其断言写入`:cancel_after_side_effect`后缀（dispatch_late_cancel_tests.rs:893–904，dispatch.rs:1380），属v4.4 Q-40过渡条款（L780），status v1落地后须改）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #194 AG-114 — AG-114: enforce evidence identity and ownership across validators [skip ci]
- 合并：merged `e9a90ad3acee6e9191cce75a3a808551b083e8f4`，merged_at 2026-10-20T17:02:05Z；验收 head `17579a391699a178f961f91f4ad4d2d6832d239b`。
- 首验材料：`out/audit-20260930/qa/codex-r7-ag114/AG114-M29-newcomplete-Root-first-acceptance-r1.json`；`out/audit-20260930/qa/codex-r7-ag114/newcomplete-controller-17579a3-M29-r1.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:2149,2152。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：本 PR 补登 `E16.6.ag114_evidence_identity_ownership`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #195 AG-144 — AG-144: Align feasibility planning with approval bounds [skip ci]
- 合并：merged `10f172eab7b97c5790bec88a3c6de2326c6e9cc5`，merged_at 2026-10-21T09:19:48Z；验收 head `0be75c234f436088a05a2ebb513b3bff2d0ddfd5`。
- 首验材料：`out/audit-20260930/qa/codex-r7-ag144/AG144-H0be75c2-PR195-Root-first-acceptance-r1.json`；`out/audit-20260930/qa/codex-r7-ag144/ctrl-0be75c2-M30-r1-runtime/eight/full-eight.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:2282。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：本 PR 补登 `E01.ag144_feasibility_approval_alignment_v43`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #196 AG-156 — AG-156: complete Q24 non-Rust CI and approved-egress checks [skip ci]
- 合并：merged `bdb4d79539c90ccbbc4d4350d473dc6e84d201bd`，merged_at 2026-10-22T07:19:52Z；验收 head `ecf2ea13e44c97fd013955548bd5e5464ae7f505`。
- 首验材料：`out/audit-20260930/qa/codex-r7-ag156/AG156-ecf2ea1-Controller-first-acceptance-r1.json`；out/audit-20260930/codex-r7-acceptance.md:2396,2399。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：只登台账（非Rust CI与外发登记（Python/工作流），无Rust测试；台账L4112有AG-156自述节但写于首验前、无PR号与合并事实）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #197 AG-120 — AG-120 Enforce one execution receipt per budget call [skip ci]
- 合并：merged `a730b9a9935bee7a41e8a47e86f329b99012566b`，merged_at 2026-10-22T19:55:32Z；验收 head `e433edc63cc6c37ff5185622fd93f5bc3e6a9201`。
- 首验材料：`out/audit-20260930/qa/codex-r7-ag120/AG120-PR197-e433edc-first-technical-acceptance-r1.json`；`out/audit-20260930/qa/codex-r7-ag120/AG120-controller-M32-r1-runtime/full-eight.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:2549。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：本 PR 补登 `E05.ag120_one_execution_receipt_per_call`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #198 AG-147 — AG-147: qualify replay terminals and versioned lineage [skip ci]
- 合并：merged `e012bba09cb662e9c01d61b52124a709106f75b8`，merged_at 2026-10-23T02:36:34Z；验收 head `79f1c7bd9e582cdf44560782fbbe75d7ef39fcc0`。
- 首验材料：`out/audit-20260930/qa/codex-r7-ag147/Root-AG147-H79-PR198-first-controller-acceptance-r1.json`；`out/audit-20260930/qa/codex-r7-ag147/AG147-M33-controller-79f1c7b-r1-runtime/full-eight.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:2730。
- GPT 二验（final-pr-matrix）：scoped_second_acceptance_pass。
- 索引：本 PR 补登 `E10.ag147_replay_lineage_versioning_v43`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

#### #199 AG-122 — AG-122: validate GA18 cycles against bound execution facts [skip ci]
- 合并：merged `a389c4e32e315ff516eba996a0a3dd40faadba0f`，merged_at 2026-10-23T11:18:51Z；验收 head `22609c6fab10bb163e6f3ec03880ef8cdfb09143`。
- 首验材料：`out/audit-20260930/qa/codex-r7-ag122/Root-AG122-H226-PR199-first-acceptance-r1.json`；`out/audit-20260930/qa/codex-r7-ag122/AG122-M34-independent-22609c6-r3-runtime/full-eight.log.test`（test 门退出 0）；out/audit-20260930/codex-r7-acceptance.md:2882。
- GPT 二验（final-pr-matrix）：scoped_pass_from_previous_phase_same_fixed_M35_and_true_M34。
- 索引：本 PR 补登 `E05.ag122_ga18_bound_cycle_validation`（见上表）。
- 状态：implemented_not_verified + 首次技术验收；不可声明项见本小节开头。

### 重跑与重验（33 条；在本 PR 首个提交 a9ad4587c5f8b70f0c6a2f6435ea8186b9a533c9 上执行，每条独立日志）

每条都是守卫下的单目标命令（`bash "$PWD/../../audit-20260930/qa/r6_guarded.sh" "$CARGO_TARGET_DIR" 0 -- cargom test --locked -p <包> --test|--lib <目标>`，cwd 为 AG-182 worktree，CARGO_TARGET_DIR 为本卡独立 target，rsproxy 镜像，非 --offline），日志首行写字面命令；新记录 plan_version v4.5 并显式 `plan_recheck: not_affected`（返修 R8-1 F4）、source_sha = a9ad4587c5f8b70f0c6a2f6435ea8186b9a533c9（本 PR 首个提交；本节所在的登记提交是其子提交，因提交无法包含自身 SHA）、pr = #200（未合并，merged_sha 为 null，事实表登记 200 => None）、command 为声明式 8 词单目标、execution.command 为字面命令；input_ref 统一为回执 `out/audit-20260930/qa/inputs/w0-input-ag-182-a9ad458.json`（SHA-256 `ac957637a78fb52e3dce6cb7d8ce6881b4e04b1d7219beb8d7aeba50f2390d89`，列出 33 份日志的字节与 SHA-256）。

| 新记录 id | E | 关系 | 旧记录 | 单目标 | 结果 | 日志 |
|---|---|---|---|---|---|---|
| E08.ag182_replay_v3_economic_cleanup | E08 | supersedes | E08.ag037_economic_replay_cleanup | evo-engine --test replay_economic_cleanup_v42 | 9 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-withdrawn/E08.ag037_economic_replay_cleanup.log` |
| E09.ag182_replay_v3_legal_prefix | E09 | supersedes | E09.ag074_local_scoped_acceptance | evo-engine --test replay_legal_prefix_v42 | 11 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-withdrawn/E09.ag074_local_scoped_acceptance.log` |
| E10.ag182_replay_v3_episode_recovery | E10 | supersedes | E10.ag064_replay_episode_recovery | evo-engine --test replay_recovery_contract_v42 | 13 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-withdrawn/E10.ag064_replay_episode_recovery.log` |
| E10.ag182_replay_v3_fairness_waits_legal_set | E10 | supersedes | E10.ag103_replay_fairness_waits_legal_set_v42 | evo-engine --test replay_fairness_waits_v42 | 9 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-withdrawn/E10.ag103_replay_fairness_waits_legal_set_v42.log` |
| E10.ag182_replay_v3_controller_acceptance | E10 | supersedes | E10.controller_acceptance | evo-engine --test replay_v41 | 13 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-withdrawn/E10.controller_acceptance.log` |
| E10.ag182_replay_v3_management_controller_acceptance | E10 | supersedes | E10.replay_management_controller_acceptance | evo-engine --test dispatch_management | 31 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-withdrawn/E10.replay_management_controller_acceptance.log` |
| E16.6.ag182_v45_project_license_metadata | E16.6 | supersedes | E16.6.ag131_v43_project_license_metadata | evo-core --test support_scope | 41 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-withdrawn/E16.6.ag131_v43_project_license_metadata.log` |
| E16.6.ag182_plan_v45_index_binding | E16.6 | supersedes=[]（note：替代 E16.6.ag012 的谱系绑定） | — | evo-core --test support_scope | 41 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-binding/E16.6.ag182_plan_v45_index_binding.log` |
| E00.ag182_rerun_controller_acceptance | E00 | completes | E00.controller_acceptance | evo-engine --lib evaluator::tests | 6 过 / 0 败，181 filtered，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E00.controller_acceptance.log` |
| E01.ag182_rerun_controller_acceptance | E01 | completes | E01.controller_acceptance | evo-core --test evaluation_v41 | 13 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E01.controller_acceptance.log` |
| E02.ag182_rerun_ag092_local_scoped_acceptance | E02 | completes | E02.ag092_local_scoped_acceptance | evo-core --test baseline_dependency_revocation_v42 | 13 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E02.ag092_local_scoped_acceptance.log` |
| E02.ag182_rerun_ag093_local_scoped_acceptance | E02 | completes | E02.ag093_local_scoped_acceptance | evo-engine --test compiler_input_binding_v42 | 5 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E02.ag093_local_scoped_acceptance.log` |
| E02.ag182_rerun_controller_acceptance | E02 | completes | E02.controller_acceptance | evo-core --test skill_edit | 14 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E02.controller_acceptance.log` |
| E03.ag182_rerun_controller_acceptance | E03 | completes | E03.controller_acceptance | evo-engine --test optimization | 7 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E03.controller_acceptance.log` |
| E04.ag182_rerun_ag077_local_scoped_acceptance | E04 | completes | E04.ag077_local_scoped_acceptance | evo-storage --test e16_budget_binding_v42 | 31 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E04.ag077_local_scoped_acceptance.log` |
| E04.ag182_rerun_controller_acceptance | E04 | completes | E04.controller_acceptance | evo-storage --test budget | 26 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E04.controller_acceptance.log` |
| E05.ag182_rerun_ag083_partition_admission_rejection_atomicity | E05 | completes | E05.ag083_partition_admission_rejection_atomicity | evo-core --test partition_admission_atomic_v42 | 31 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E05.ag083_partition_admission_rejection_atomicity.log` |
| E05.ag182_rerun_ag087_local_scoped_acceptance | E05 | completes | E05.ag087_local_scoped_acceptance | evo-engine --test formal_candidate_dependency_binding_v42 | 27 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E05.ag087_local_scoped_acceptance.log` |
| E05.ag182_rerun_ag088_local_scoped_acceptance | E05 | completes | E05.ag088_local_scoped_acceptance | evo-engine --test complete_batch_statistics_binding_v42 | 2 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E05.ag088_local_scoped_acceptance.log` |
| E05.ag182_rerun_ag089_local_scoped_acceptance | E05 | completes | E05.ag089_local_scoped_acceptance | evo-engine --test early_stop_certificate_binding_v42 | 10 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E05.ag089_local_scoped_acceptance.log` |
| E05.ag182_rerun_ag090_local_scoped_acceptance | E05 | completes | E05.ag090_local_scoped_acceptance | evo-engine --test invalid_formal_reason_binding_v42 | 7 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E05.ag090_local_scoped_acceptance.log` |
| E05.ag182_rerun_ag091_local_scoped_acceptance | E05 | completes | E05.ag091_local_scoped_acceptance | evo-engine --test late_grader_stop_preservation_v42 | 8 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E05.ag091_local_scoped_acceptance.log` |
| E05.ag182_rerun_controller_acceptance | E05 | completes | E05.controller_acceptance | evo-engine --test streaming_evaluator | 14 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E05.controller_acceptance.log` |
| E06.ag182_rerun_ag084_local_scoped_acceptance | E06 | completes | E06.ag084_local_scoped_acceptance | evo-engine --test candidate_review | 16 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E06.ag084_local_scoped_acceptance.log` |
| E07.ag182_rerun_controller_acceptance | E07 | completes | E07.controller_acceptance | evo-http --test service | 7 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E07.controller_acceptance.log` |
| E08.ag182_rerun_ag075_local_scoped_acceptance | E08 | completes | E08.ag075_local_scoped_acceptance | evo-storage --test dependency_read_v42 | 4 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E08.ag075_local_scoped_acceptance.log` |
| E08.ag182_rerun_controller_acceptance | E08 | completes | E08.controller_acceptance | evo-storage --test lifecycle | 23 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E08.controller_acceptance.log` |
| E09.ag182_rerun_controller_acceptance | E09 | completes | E09.controller_acceptance | evo-engine --test exploration_v41 | 5 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E09.controller_acceptance.log` |
| E11.ag182_rerun_controller_acceptance | E11 | completes | E11.controller_acceptance | evo-core --test replay_economics | 4 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E11.controller_acceptance.log` |
| E12.ag182_rerun_controller_acceptance | E12 | completes | E12.controller_acceptance | evo-engine --test curriculum_v41 | 3 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E12.controller_acceptance.log` |
| E16.1.ag182_rerun_ag070_local_scoped_acceptance | E16.1 | completes | E16.1.ag070_local_scoped_acceptance | evo-engine --test import_development_material_v42 | 12 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E16.1.ag070_local_scoped_acceptance.log` |
| E16.1.ag182_rerun_ag076_local_scoped_acceptance | E16.1 | completes | E16.1.ag076_local_scoped_acceptance | evo-core --test import_fixture_v42 | 34 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E16.1.ag076_local_scoped_acceptance.log` |
| E16.6.ag182_rerun_ag086_local_scoped_acceptance | E16.6 | completes | E16.6.ag086_local_scoped_acceptance | evo-core --test support_scope | 41 过 / 0 败，exit 0 | `out/audit-20260930/qa/claude-r8-rerun-missing/E16.6.ag086_local_scoped_acceptance.log` |

- 六条回放撤出记录与 E16.6.ag131 的 withdrawn_records 状态由 pending_recheck 改为 superseded，并由新记录 supersedes 指向；新记录 scope 只声明旧记录原作用域（E10.replay_management 只计 replay.run 五例与原八个通用作业用例；E10.ag064 的“…remain_verified_and_idempotent”测试名旧实新，按断言内容登记）；回放新记录的 related_evidence_refs 按撤出前的导航位置恢复（只限其 E 在映射内的条目），E16.6.ag182_v45_project_license_metadata 接回 V116，E16.6.ag182_plan_v45_index_binding 登记于 V071/V080。
- 25 条缺件记录的重跑以 completes=[旧 id] 登记，旧记录及其登记路径/摘要不改写、不撤出；计数按本次运行如实登记（后来加入同一目标的用例不扩成旧记录的信用）。三条 E05 撤出记录（E05.ag052/ag080/ag101）不在本表，由 AG-192 在新判据下重验。

### 计数与不可声明项

- verified_subscopes：98 − 5（撤出）+ 12（补登，含有条件 2）+ 33（本节）= 138；withdrawn_records 24（pending_recheck 16、superseded 7、replaced_by_binding 1）；缺失清单 42 路径 / 25 条记录。
- 返修 R8-1 的钉扎计数：撤出元组 24、关系对 32（supersedes 7 + completes 25）、历史记录摘要 93（= 138 − 45 条本 PR 新记录）、缺失清单三元组 42；plan_recheck 分布：rechecked_v4.5 91（79 条保留并集记录 + 12 条补登）、rechecked_v4.3 11、not_affected 显式 35（33 条本 PR 新记录 + E17/E18 两条）、缺省 1（E16.4.ag126）。
- 不可声明：二次验收、完整 V071/V080/V116/V123、E16.6 或 E16 关闭、真实宿主/沙箱/provider、付费实验、任何效果或收益、发行；本 PR 状态最高 implemented_not_verified，首次技术验收由 W0 另行执行。

## 第八阶段 AG-200：检查器加固与 PR #200 合并事实回填

本节登记 E16.6 增量 AG-200（分支 `claude/ag-200-checker-hardening-r8`，基线 origin/main `02a135aa918fce5dd2ea0c1c8eafc287318f10d2`）：来源为 AG-182 独立审查 R-02/R-05/R-09/S-1/S-2/S-3 的后续与 PR #200 合并事实的回填。真源条款：§18.10 L3142（机器口径）与 L3159（本地缺失的证据路径：清单外缺失判失败，已登记路径重现而字节或摘要不符判失败，unknown 重现一律判失败）、§13.1 L2123/L2129（V080 与 V080.d 第 3/4/5 类）、§18.5 L3071（验证记录最小字段）、§18.11 L3170（检查器与索引测试由 E16.6 增量 PR 更新）。状态最高 implemented_not_verified；W0 验收（十一门 rev2、探针集、独立只读审查）与管理员合并尚未发生，GPT/Claude 二次验收 not_executed。本卡不新增验证记录。

### (a) Python 检查器缺省失败关闭（提交一）

- `scripts/check_support_scope.py`：无 `--evidence-root` 时，out/ 内无法判定的证据路径（未登记缺失 + 已登记未查验，两者合计记入报告新键 `evidence_unjudged`）在逐条判定全部完成之后使检查失败：`evidence store not supplied: <n> out/ evidence paths unjudged; pass --evidence-root <primary checkout> or --structure-only`，退出 1；逐条判定中的摘要不符、重现失配、清单外缺失仍先报告。显式 `--structure-only` 只查结构并退出 0，报告 `evidence_judgment: not_performed_structure_only`；有证据根且判定完成为 `judged`，判定未完成为 `not_performed`。`--structure-only` 与 `--evidence-root` 互斥（argparse 用法错误，退出 2）；两个 `--*-only` 子模式拒绝 `--structure-only`。报告顶层键由 12 个变为 14 个。
- 本 worktree 三模式实测（`--repo-root . --manifest reports/support-scope.json --source-of-truth <v4.5 真源>`）：缺省 → 退出 1，errors 仅上述一句（n = 282，即 240 条未登记 + 42 条已登记），evidence_judgment not_performed；`--structure-only` → 退出 0，structure_valid true，not_performed_structure_only，evidence_unjudged 282；`--evidence-root <主检出>` → 退出 0，errors []，evidence_missing 42，evidence_unjudged 0，judged。
- CI 不携带 out/：CI 与 non_rust_delivery 中的 unittest 里涉及清单检查的用例（临时夹具与真实仓根）一律显式 `--structure-only`，缺省失败关闭与证据根判定另有临时夹具专测；不声明“CI 已判定 out/ 证据”。
- 返修 R8-1（提交三，审查 F-04）：`evidence_unjudged` 改为在逐条判定中随遇随计；判定中途因重现、摘要不符或清单外缺失而失败时，报告给出失败前已累计的计数（此前恒为 0）。field_semantics.evidence_judgment 在三处同步补一句“evidence_unjudged counts the unjudged paths as they are met, so a check that stops early reports the count reached”，其余 16 条不变。用例 `V45RebindTests.test_early_exit_reports_the_unjudged_paths_counted_so_far`。

### (b) Rust 证据库输入（提交一）

- `crates/evo-core/tests/support_scope.rs`：`validate_missing_register` 增加显式参数 `evidence_root: Option<&Path>`，经新增的 `validate_manifest_with_evidence` 透传；`validate_manifest`/`validate_manifest_with` 保持 None。只有校验真实 `reports/support-scope.json` 的顶层用例读取环境变量 `RSIA_SUPPORT_SCOPE_EVIDENCE_ROOT`（须为已存在的绝对目录，否则用例失败）；未设置时只做结构检查并在测试输出打印 `evidence judgment not performed (set RSIA_SUPPORT_SCOPE_EVIDENCE_ROOT)`。
- 有证据根时 out/ 路径在证据根下判定，与 Python 同三条规则：清单外缺失失败；已登记路径重现而字节或 SHA-256 不符失败；unknown 身份重现一律失败；现存 input_ref 须与记录 input_digest 一致（与 Python 对齐，有无证据根均适用）。新用例 `missing_evidence_register_judges_out_paths_against_an_evidence_store` 以临时目录伪造证据库：完整库通过；未登记缺失、登记路径重现失配、unknown 重现、input 摘要不符各断言错误子串；“删一登一（计数不变）”依次被清单文件 SHA、三元组、证据库判定拒绝；无证据根时同样的 out/ 缺失与库内重现不判定。真实 out/ 只读不写。

### (d) PR #200 合并事实回填与 138 条摘要钉扎（提交一）

- 33 条 pr=#200 记录写入 `merged_sha` `02a135aa918fce5dd2ea0c1c8eafc287318f10d2` 与 `pr_state: merged`（字段集与 105 条已合并记录一致，pr_state 紧随 merged_sha），其余字段逐字节不变；合并事实表 `200 => Some("02a135aa918fce5dd2ea0c1c8eafc287318f10d2"),`（保持每行字面形态，Python 解析同一表），表中不再有 None。
- 两侧 EXPECTED_RECORD_DIGESTS 由 93 条扩为 138 条（按索引顺序；新增 45 条 = 12 条补登 + 33 条 PR #200 记录，按回填后内容、现行规范化规则计算），钉扎覆盖全部现存记录；33/12 的计数断言不变。
- 派生索引文字：E16.6 status_reason 的钉扎口径改为“全部 138 条记录的摘要、42 个三元组与清单文件 SHA-256”；E16.6 remaining[4] 中 “left to AG-200” 一句按落地事实改写（缺省失败关闭、`--structure-only`、Rust 环境变量证据库输入）；remaining[3] 的 AG131 句改为指向现行记录 E16.6.ag182_v45_project_license_metadata（R-13 残留）。derived_index_notice 不含“缺省只提示”措辞，未改；'V001-V120' 字样在白名单文件中 0 处（README 不在白名单，本台账 v4.3 阶段的 V001–V120 是历史原文，均未改）。

### (e) 缺失清单文件身份钉扎（提交一）

- 两套检查器各钉常量 `EXPECTED_MISSING_REGISTER_SHA256` = `b843e3f9692c5ba18ce9148278b3267584222461406fdccc17162ed84c115337`（reports/evidence-missing.json 现行字节，24,047 字节，本卡未改该文件），要求 manifest 指针 sha256 == 常量 == 文件实际 SHA-256；Python 另钉清单路径（`--known-missing` 非 `reports/evidence-missing.json` 即拒绝，与 Rust 对齐）；三元组钉扎保留。原“改身份并刷新指针即通过”的用例改为拒绝向（探针 c4：unknown 改伪造 SHA 并放回伪造文件；c5：已知身份改 unknown），仅当两侧常量同时重钉（模拟经审查的清单更新）时通过。W0 裁决：不移除 rechecked_v4.2（§18.10 L3142 明文保留历史值）。
- field_semantics 追加 4 条（两套检查器与派生索引三处逐字一致，既有 13 条原文不变，共 17 条）：`evidence_judgment`（judged / not_performed_structure_only / not_performed、`--structure-only` 与缺省失败关闭、Rust 的 `RSIA_SUPPORT_SCOPE_EVIDENCE_ROOT`）、`evidence_missing_register_pin`（清单文件 SHA-256 与路径钉死）、`pr_state`、`merged_sha`。

### (c) Python 独有检查移植到 Rust 与错误子串（提交二）

以 R1 侦察 §Q4 清单为准逐项移植；每项两侧各有断言错误子串的拒绝用例（Python 原缺的拒绝用例一并补齐）。

| R1 §Q4 | 检查 | Rust 落点（crates/evo-core/tests/support_scope.rs） | Python 拒绝用例 | Rust 拒绝用例 |
|---|---|---|---|---|
| 1–3 | 闭合键集：manifest root、dimensions、trace_index_coverage、scope 条目、证据记录、traceability、B/U/K、SO、local_use、V | `known_keys` + ROOT_KEYS 等十个常量（与 Python 同集） | `test_rejects_unknown_keys_asserting_effect_or_verdict`（十处） | `rejects_unknown_keys_in_every_closed_object`（同十处） |
| 4 | log_path 全局唯一 | `EvidenceClaims::claim` | `test_rejects_evidence_duplicated_or_borrowed_from_another_task` | `rejects_duplicated_runs_and_inconsistent_merge_facts_across_records` |
| 5 | 跨任务同一运行 (source_sha, test_entry)；同 PR 同 merged_sha；一个合并提交只合并一个 PR；merged_sha 不是他 PR 的 source_sha，source_sha 不是他 PR 的合并提交 | `EvidenceClaims::claim`；合并事实表比对移到全部记录之后（与 Python 同序） | 同上 + `test_rejects_merged_sha_inconsistent_with_manifest_merge_facts` + `RustParityRejectionTests.test_source_sha_recorded_as_another_prs_merge_commit_is_rejected` | 同上 |
| 6 | 现存 input_ref 比 input_digest | `validate_missing_register`（提交一，有无证据根均适用） | `test_missing_input_is_needs_verification_but_wrong_hash_is_error` | `missing_evidence_register_judges_out_paths_against_an_evidence_store` |
| 7 | pr_state ∈ {draft, open, closed, merged}；merged ⇔ merged_sha 非空 | `validate_evidence_record` | `test_rejects_merged_sha_inconsistent_with_manifest_merge_facts` + `RustParityRejectionTests.test_rejects_pr_state_outside_the_registered_set` | `pr_state_agrees_with_merged_sha` |
| 8 | completion_evidence_refs：非 verified 的 scope 不得带；verified scope ⊆ 自有记录；SO/V/B/U/K 的 verified 须有 completion ⊆ refs（V 另须无未完成范围） | `validate_e_scopes`、`validate_so_sources`、`validate_v_scenarios`、`validate_commitments` | `RustParityRejectionTests.test_completion_evidence_refs_follow_the_status` | `completion_evidence_refs_follow_the_status` |
| 9 | withdrawn_records 键闭合与 status（两侧原已等价）；数字只收 ASCII | Python `PR_RE`/`OWNER_TASK_RE`/`ISO_UTC` 的 `\d` 改 `[0-9]`；Rust `valid_owner_task` 遇多字节字符不再 panic | `test_withdrawn_records_are_the_reviewed_list_and_ids_never_revive` + `RustParityRejectionTests.test_non_ascii_digits_are_rejected_like_the_rust_checker` | `withdrawn_records_are_the_reviewed_list_and_never_revive` + `non_ascii_digits_are_rejected_like_the_python_checker` |
| 10 | planned 不得带记录；optional_disabled 不得 verified；explicitly_out_of_scope 须登记；父子状态秩；implementation_files 去重；V 两轴一致 | `validate_e_scopes`（STATUS_RANK）、`validate_v_scenarios` | `ControllerFalseAcceptanceTests`（planned、父子秩、optional）+ `RustParityRejectionTests.test_scope_and_scenario_status_rules` | `scope_and_scenario_status_rules_match_the_python_checker` |
| 补（R8-1，提交三） | R1 §Q4 漏列的四条：B/U/K 的 planned 实现须配 planned 验证、verified 验证不得配 planned/blocked 实现；B/U/K `local_artifacts` 去重；SO `local_use.artifacts` 去重。另 V 场景 remaining 与 Python 同口径：非 verified 须非空、verified 须为空 | `validate_commitments`、`validate_so_sources`、`validate_v_scenarios` | 四条规则原已在 Python（Python 侧未改）；V remaining 两向补 `RustParityRejectionTests.test_v_scenario_remaining_follows_the_status` | `commitment_axes_and_artifact_lists_match_the_python_checker`、`v_scenario_remaining_follows_the_status_like_the_python_checker` |

- R1 §3.4 列出的 54 处裸 `is_err()`（27 个测试）全部改为 `expect_err` + 错误子串断言；文件中 `is_err()` 剩余 0 处（W0 static_check 的单行启发式计数亦为 0）。
- 同时修正：Python 对非字符串 pr_state 不再抛未捕获 TypeError；Rust 对非 ASCII owner_task 不再 panic（`str::split_at` 落在多字节字符内）。
- 残留（如实登记；返修 R8-1 后更正）：提交二时登记为“仍只在 Python”的 B/U/K 四轴与两处 artifacts 去重已由提交三移植（见上表“补（R8-1）”行）。提交二所写“Rust 独有且更严的规则保持不变”一句不成立：B09/K06 的 implementation_status 在 Rust 钉为 planned，但其 verification_status 在提交三之前 Rust 并不约束，现由四轴规则约束。V 场景 remaining 已与 Python 同口径，verified 的 V 场景在两侧同样可以通过。仍未移植的 Python 独有检查只剩一处：Python 的 string_set 拒绝空串元素，Rust 的 string_set 不拒；其余数组另有集合比对或引用校验兜底，唯一会被 Rust 放行的是 `explicitly_out_of_scope_until_revised` 含空串（现为空列表）。Rust 独有、只会多拒的规则：B/U/K 的 implementation_status 逐条钉死、SO 的 license_status/verification_status/local_use.mode 钉死、B/U/K 的 remaining 恒须非空（Python 只要求非 verified 者非空）。Python 的 scope/V/B/U/K status 若为不可哈希值仍会抛未捕获 TypeError（退出 1 带回溯，本卡只修了 pr_state）。

### 探针与验收（W0）

- W0 验收脚本 qa/claude-r8-prep/ag200-accept/probes.py（实施者未运行；它会对 reports/*.json 做 checkout）：AG-182 的 13 个（a、b、c、c2、c3、d、e、f1、f2、g1、h1、k1、r1）+ a1（缺省无参须失败）、a2（`--structure-only` 须通过且标注）、c4、c5、j1、j2、k4、k5、m1、m2。对应单测：a1/a2 → `FailClosedEvidenceJudgmentTests`；c4/c5 → Python `test_register_file_sha256_is_pinned_in_both_checkers`、Rust `missing_evidence_register_is_pinned_and_decides_local_outcomes`；j1/j2 → Python `test_registered_missing_paths_are_listed_and_reappearance_is_checked`、Rust `missing_evidence_register_judges_out_paths_against_an_evidence_store`；k4/k5/m2 → 138 条摘要钉扎；m1 → 合并事实表、跨记录合并事实与摘要钉扎。
- W0 静态核对 qa/claude-r8-prep/ag200-accept/static_check.py（只读，实施者已跑）：PROBLEMS 0；rust is_err-only assert lines 0；两侧 EXPECTED_RECORD_DIGESTS 各 138 条，与按记录独立重算一致；清单文件 SHA-256 == 指针 == 两侧常量；33 条 PR #200 记录只有 merged_sha/pr_state 变化，其余 105 条逐字节不变。

### 自测（实施者，head 为提交二）

- `cargom fmt --all -- --check`：退出 0（提交一、提交二各一次）。
- `cargom clippy --workspace --all-targets --locked -- -D warnings`：退出 0（提交一、提交二各一次；首轮在提交二上报出提交一新增的测试辅助函数 type_complexity，已在提交一内改用类型别名后两处重跑）。
- `cargom test --locked -p evo-core --test support_scope`：基线 43；提交一 44 过 / 0 败（未设变量，测试输出 `evidence judgment not performed (set RSIA_SUPPORT_SCOPE_EVIDENCE_ROOT)`）与 44 过 / 0 败（`RSIA_SUPPORT_SCOPE_EVIDENCE_ROOT=<主检出>`）；提交二 50 过 / 0 败（两种方式各一次；新增 1 + 6 个测试）。
- `python3 -m unittest scripts.test_check_support_scope`：基线 106；提交一 112 OK；提交二 117 OK（新增 6 + 5 个测试）。
- 检查器三模式（提交二，`--repo-root . --manifest reports/support-scope.json --source-of-truth <v4.5 真源>`）：缺省退出 1，errors 仅 `evidence store not supplied: 282 out/ evidence paths unjudged; …`；`--structure-only` 退出 0，not_performed_structure_only，evidence_unjudged 282；`--evidence-root <主检出>` 退出 0，errors []，evidence_missing 42，evidence_unjudged 0，judged；两旗同用退出 2；报告 14 键。
- `python3 scripts/check_build_inputs.py` 0、`python3 scripts/check_dependency_register.py` 0、`python3 -B scripts/non_rust_delivery.py` 0（提交二的干净检出）。
- 未跑：全量 workspace 测试（卡未要求，W0 十一门覆盖）、`cargom build -p rsia` 与 smoke_cli/smoke_workspace、W0 的 probes.py。

### 不可声明项

GPT/Claude 二次验收；完整 V071/V080/V116；E16.6 关闭；任何效果；“CI 环境已判定 out/ 证据”（CI 无 out/，只做结构检查，报告与测试输出如实标注）。本卡不新增验证记录，状态最高 implemented_not_verified。
