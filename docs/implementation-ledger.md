# RSIAgent 实施台账

规范入口（本地，不进远程）：`RSIAgent-v4定稿-工程实施方案-2026-09-17.md`。
plan_version：v4。规范正文 SHA-256：`d26ab3506736849f3ec1d286b49fcfa581a09c8be2243681fcc8e93b758f9d6d`。
本台账只索引该规范的任务与验收，不能独立修改门禁。
v3.3（SHA-256 `1b587034…e247294a`）及更早方案自 v4 起在规范意义上被替代，原件复制归档于 `archive/2026-09-17-pre-v4/`（本机，gitignored），历史事实保留但无执行权。

仓库：`acosmi/RSIAgent`。日期：2026-09-18 UTC（v4 切换；下表早期行仍记录 v3.3 时代的动作）。

状态值：`planned` / `in_progress` / `blocked` / `implemented_not_verified` / `verified` / `explicitly_out_of_scope`。
`verified` 不表示效果 improved。效果结论和部署支持另列。

远程仓库只进源码。方案正文、物理归档与本机 `archive/` 不发布。

## 当前工作树事实

| 项 | 值 |
|---|---|
| E00 开始时 origin/main | `6e83d1285b778a62243446bbe0a1eb14809de4f3` |
| 方案正文 SHA-256 | `1b5870341be7c027c369a08c6e6bfd8f06dc494fc7174fff8930f42ae247294a` |
| 原始本地包 SHA-256 | `63c2d194e5371b2eff3e2e38fa714a2ec51334b1a934f64a5027e0399b813b62` — **blocked_not_found** |
| 增量包 | `RSIAgent-v3-source-increment-20260916.zip` SHA-256 `a8e0a6226c354969db630529ff90a0e934b608825912cd3e71593dcbfa7e05cb` — **blocked_not_found** |
| 增量源码提交 | `b9f0124896c860fb670d0d194d9f59339e7d78c0` — 远程与本地对象库均不存在 |
| 打包提交 | `78ad9f8bde92fdd123e3065af77022c60b4d0c75` — 不存在 |
| 既有迁移 | `crates/evo-storage/migrations/0001_runtime.sql` |
| 下一空号 | `0002`。不得创建 `0003_exploration_worlds.sql` 占用 0003。增量自称占用的 `0003_v3_assets.sql` 因缺包未归并。 |
| 拒绝的旧 H00 分支 | `implementation/v3-h00-reconcile-20260916` @ `e659916`（用二进制块重建，违反“缺包不得从方案复造源码”） |

## 主任务

| 编号 | 范围 | 状态 | 证据 / 退出条件 |
|---|---|---|---|
| E00 | 归并真实源码与可重建输入 | verified（核心基线）/ blocked（历史归并子项） | v4：PR #27 merged `90b4069`（squash），CI verify pass，本地门禁全 0（126 测试）；§1.4 六条缺口全部确认仍存在，v4 全部升级义务 planned。V073 历史包归并仍 blocked_not_found，不隐藏该限制。v3.3：PR #3 merged `5e40b0b…`。 |
| E01 | 冻结实验、任务分区和预算可行性 | implemented_not_verified | PR #5 merged `a62451a20e16b8f5cf58ca0200afb90676237058`。付费小试未授权。远程 CI billing-locked。 |
| E02 | 最小版本化契约与宿主能力边界 | implemented_not_verified | PR #6 merged `991d47efb35bd044c629df4548b9d97d943b1773`。远程 CI billing-locked。 |
| E03 | 跨任务证据接入生成消费者 | implemented_not_verified | PR #7 merged `fec2f32b3c4ff860fb845a08a3864a723c87b30a`。 |
| E04 | 可信执行、隔离与根资源预算 | implemented_not_verified | PR #8 merged `fb5f50e12437a817e7fe51f1098474589d22ce50`。 |
| E05 | 独立验收器与有边界统计判定 | implemented_not_verified | PR #9 merged `d2d32c63a82322738be562e0b47a2bd696a192f8`。 |
| E06 | 组合发布、实际应用与最小撤销闭环 | implemented_not_verified | PR #10 merged `18057b4fd81def2a76566c5d1f8c691f578f5b18`。 |
| E07 | 第一个最小可验证真实闭环 | implemented_not_verified | PR #11 merged `38b4f39eeee6ece7b28324d0d915eda0ceab9ab4`。真实模型仍 blocked。 |
| E08 | 可恢复的撤销、保留和备份链 | implemented_not_verified | PR #12 merged `cfc6a542cec4779ffce500c917d1fbf656d0e71e`。 |
| E09 | 生成/探索解耦与有状态在线探索 | implemented_not_verified | PR #13 merged `adf431959cadb0d41bd6b1978873fe964494a098`。 |
| E10 | 不可变世界池与纯查表回放 | implemented_not_verified | PR #14 merged `9b76d6e2f5112b70b2d6ac2ce167ec7f3144c540`。 |
| E11 | 验证回放优化的真实经济收益 | implemented_not_verified | PR #15 merged `8ef4c8d37bf2323061021c82465163af66c1c8b2`。真实配对未跑。 |
| E12 | 学习者条件化的经验自主获取 | implemented_not_verified | PR #16 merged `b9cb2c1fc0ffe0e2c52a5a1547c5d75552988b0a`。 |
| E13 | 长期部署适应与能力保留监测 | implemented_not_verified | PR #17 merged `df44cb8b4b90556d1ce32fe4889a8ff7b37eaf49`。 |
| E14 | 受限改进器自身的继承控制器 | implemented_not_verified | PR #18 merged `d6f5742922eee76971356be56e80b609e0dccbf2`。 |
| E15 | 后继质量与跨代收益实验 | implemented_not_verified | PR #19 merged `3ef355138815c4920ac4d414ad11a041392d9f4a`。 |
| E16.1 | 来源导入与版本化读取器 | implemented_not_verified | PR #20 merged `c45eaebc9cd3eae9dd8f18c1a40d4ff4dd0a3334`。 |
| E16.2 | 资产导入/分享与隐私门禁 | implemented_not_verified | PR #21 merged `1431d174e1ee4e4540e568425cf5321e259a62ee`。 |
| E16.3 | 内置种子与本地修改保护 | implemented_not_verified | PR #22 merged `117629751fd45c6281dd08ddd4ba34bec3ca20eb`。 |
| E16.4 | 额外真实宿主与配置面漂移 | blocked | PR #23 merged `afd49bbbfff64be4f6cf5e7c9f16dd1367af13fd`。Claude Code 未安装，保持 blocked。 |
| E16.5 | 持久恢复、容量、依赖与部署安全 | implemented_not_verified | PR #24 merged `e18cbaae761236671d50f44da0e83bb26b76d602`。 |
| E16.6 | 发行、证据台账与唯一真源交接 | implemented_not_verified | PR #25 merged `5c74aff0bdd908b8f25f84fbd18c1799b7f2b1f6`。 |
| E17 | 可选：开发代理评分器演化 | implemented_not_verified | 默认关闭；`enable_agent_scorer_evolution(true)` 拒绝。未修订方案不得打开。 |
| E18 | 可选：自动提出代码修改 | implemented_not_verified | 默认关闭；`enable_automatic_code_prs(true)` 拒绝。 |

## B01–B10

| ID | 规范纳入 | 实现 | 验证 | 效果 |
|---|---|---|---|---|
| B01 | yes | no | not_run | not_claimed |
| B02 | yes | no | not_run | not_claimed |
| B03 | yes | no | not_run | not_claimed |
| B04 | yes | no | not_run | not_claimed |
| B05 | yes | no | not_run | not_claimed |
| B06 | yes | no | not_run | not_claimed |
| B07 | yes | no | not_run | not_claimed |
| B08 | yes | no | not_run | not_claimed |
| B09 | yes | no | not_run | not_claimed |
| B10 | yes | no | not_run | not_claimed |

## 作用域化 V 场景（E00）

| 场景 | 状态 | 说明 |
|---|---|---|
| V001@E00 | implemented_not_verified | v3.3：HEAD `6e83d12`，本地门禁 0。v4：HEAD `559d4da`，fmt/check/test(126)/clippy/build/smoke×2 全 0，日志在 `out/e00-v4/`；远程 CI 沿用 billing-locked 记录。 |
| V071@E00 | implemented_not_verified | v4：台账入口切换为 `RSIAgent-v4定稿-工程实施方案-2026-09-17.md`（SHA-256 `d26ab350…58f9d6d`）；v3.3 原字节复制归档至 `archive/2026-09-17-pre-v4/`（含 manifest）；`.gitignore` 继续阻止方案与 archive 进远程；无需要求回读旧方案的跳转。 |
| V072@E00 | implemented_not_verified | v3.3：清点 20 项。v4：清点 112 项实际测试，全部无 T 编号、`legacy_equivalence_unverified`；未删断言。 |
| V073@E00 | blocked | 原包与增量包缺失，不认证来源、不默认复制、不从方案复造。独立实现可继续。v3.3 与 v4 结论相同。 |
| V080@E00 | implemented_not_verified | v4：映射写入本台账；§1.4 六缺口逐项复核仍存在；B01–B10/U01–U08 与 E/V 链的 v4 增量义务全部 planned；计划/来源/静态测试 ≠ 已实现/已运行。 |
| V010@E01 | implemented_not_verified | 计划冻结后才能绑候选、开查询账本；失败/取消不退还新种子。 |
| V011@E01 | implemented_not_verified | raw hash、规范化近重复、family 跨 development/acceptance 均 Conflict。 |
| V012@E01 | implemented_not_verified | n<2、NaN、重复簇、单次 alpha 分配、边界 micros。v1 `evaluate` 保留。 |
| V013@E01 | implemented_not_verified | 固定零效应不得 Improved；已知退化 Regressed；估算不可行时不降阈值。仿真不是产品收益。 |
| V002@E02 | implemented_not_verified | v1 五项 deny_unknown_fields 仍拒绝伪造身份字段；四工具名称固定。 |
| V003@E02 | implemented_not_verified | offered⊇attached⊇used⊇verified_benefit；Tool-only 不得报 attached/used。 |
| V009@E02 | implemented_not_verified | 管理/评测操作不在模型工具列表；HTTP body 不能自造 actor/role。 |
| V043@E02 | implemented_not_verified | inherit / reset_to_baseline / set（含显式空串）语义不同；裸 null 拒绝。 |
| V044@E02 | implemented_not_verified | FieldContract 静态表；无消费者字段与候选写保护核心均拒绝。 |
| V045@E02 | implemented_not_verified | 缺省 inherit；reset 读 B 不是宿主当前默认。 |
| V046@E02 | implemented_not_verified | 未知 settings 透传拒绝。 |
| V047@E02 | implemented_not_verified | 关闭进化时投影不改宿主工具列表、不注入指令。 |
| V048@E02 | implemented_not_verified | 同槽位多写者 conflicting_writers。 |
| V060@E02 | implemented_not_verified | 参考宿主 fixture 分类 supported/runtime_owned/unsupported。 |
| V062@E02 | implemented_not_verified | supported 无 consumer 拒绝；空提取失败。 |
| V078@E02 | implemented_not_verified | 撤销依赖使 reset/inherit 结果仍拒绝。 |
| V079@E02 | implemented_not_verified | 相同 P/B/补丁得到相同 bundle digest。 |
| V004@E03 | implemented_not_verified | 两个已授权 run 进入 ModelEvidenceRequest.source_ids。 |
| V005@E03 | implemented_not_verified | task_origin / execution_attestation / purpose 分轴。 |
| V006@E03 | implemented_not_verified | 日志中的命令与 `..` 不成为新来源。 |
| V017@E03 | implemented_not_verified | 撤销非 primary 来源使 pending jobs 失效。 |
| V051@E03 | implemented_not_verified | 未授权根与路径穿越 Forbidden。 |
| V052@E03 | implemented_not_verified | 任一截断维使 coverage 为 partial，不是单一 complete。 |
| V054@E03 | implemented_not_verified | 导入 attestation 不能升格为 TrustedHost。 |
| V055@E03 | implemented_not_verified | 单 run 不能当作跨任务生成输入。 |
| V057@E03 | implemented_not_verified | preference/environment 不进入 SkillGenerator。 |
| V058@E03 | implemented_not_verified | improvement_method 在 meta 未开时 blocked_feature。 |
| V007@E04 | implemented_not_verified | answers/db/docker.sock/credentials 路径拒绝。 |
| V008@E04 | implemented_not_verified | 同 billing_scope 不能拆根；超时 uncertain 不释放不重发；旧 lease fence。 |
| V009@E04 | implemented_not_verified | 未授权模型出站拒绝。 |
| V038@E04 | implemented_not_verified | sandbox_unavailable 且禁止宿主 shell 兜底。 |
| V010@E05 | implemented_not_verified | 冻结计划后扣查询；不完整执行 fail ticket 且不退还。 |
| V012@E05 | implemented_not_verified | 正式结论走 empirical_bernstein.v2。 |
| V013@E05 | implemented_not_verified | 零效应 FormalEvaluation 不得 Improved。 |
| V028@E05 | implemented_not_verified | ReplayReport 不能转换为 FormalEvaluation。 |
| V014@E06 | implemented_not_verified | 同父 CAS 一胜一冲突。 |
| V015@E06 | implemented_not_verified | 仅 Admin 可批准；Evaluator 不能自批。 |
| V016@E06 | implemented_not_verified | 撤销后新回执拒绝；副作用声明保留。 |
| V047@E06 | implemented_not_verified | Tool-only 回执不得报 used。 |
| V003@E07 | implemented_not_verified | Tool-only 回执无 used/verified_benefit。 |
| V010@E07 | implemented_not_verified | 闭环使用冻结 ExperimentPlan 与查询账本。 |
| V014@E07 | implemented_not_verified | 无凭据时即使仿真 Improved 也不 Active。 |
| V042@E07 | implemented_not_verified | 零效应完整保留为 Inconclusive，不自动晋级。 |
| V017@E08 | implemented_not_verified | 依赖边查出后继；撤销水位跨 namespace 隔离。 |
| V018@E08 | implemented_not_verified | restore 脚本在缺 watermark 表/行时 isolate，不覆盖 dest。 |

## 历史映射（仅追踪）

H00–H24 只保留 v3.3 §18.3 的映射。历史完成状态不继承。T001–T126 原文未提供，不编造。

## 基线测试清点方法

```text
python3 scripts/inventory_baseline_tests.py
```

旧报告的 90/15/12/8 数量不是本轮清单或通过结果。已有回归测试不得删除或改 fixture 只为变绿。

## E00 本地命令记录（合并前）

输入 HEAD：`6e83d1285b778a62243446bbe0a1eb14809de4f3`。toolchain：rustc 1.98.1。本机 `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk`。

| 命令 | 退出码 | 日志 |
|---|---|---|
| `cargo fmt --all -- --check` | 0（格式化后） | `out/e00/fmt-after.txt`（本地，不入库） |
| `cargo generate-lockfile` | 0 | `Cargo.lock` 入库 |
| `cargo check --workspace --all-targets --locked` | 0 | `out/e00/check.txt` |
| `cargo test --workspace --locked` | 0 | 20 项通过 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | `out/e00/clippy.txt` |
| `cargo build --locked -p rsia` | 0 | `target/debug/rsia` |
| `python3 scripts/smoke_workspace.py` | 0 | `SMOKE_WORKSPACE_OK`；next_free_migration=0002 |
| `python3 scripts/smoke_cli.py target/debug/rsia` | 0 | `SMOKE_CLI_OK` |
| `python3 scripts/inventory_baseline_tests.py` | 0 | 20 tests |

回滚点：丢弃本分支；main 仍为 `6e83d12`。合并后走回退 PR，不强推历史。

## v4 E00 复核（plan_version=v4，base `559d4da2f3c9ffd0e34a442bccd1d444214f69f5`）

### §1.4 逐项复核结果（本轮直接读取上述六个文件确认，不是沿用 v3.3 结论）

| 定位 | 结论 | 证据 |
|---|---|---|
| `evo-engine/src/exploration.rs` + `evo-core/src/strategy.rs` | 缺口仍存在 | `Coordinator::decide()`（exploration.rs:64-66）只 advance Observed→Selected；`PrefixView`（strategy.rs:86-90）仅 search_parent/approved_parent/depth，无质量/失败/预算字段；无 LegalActions/BudgetView/批次动作/真实 dispatch 消费者。 |
| `evo-engine/src/replay.rs` | 缺口仍存在 | `lookup()`（replay.rs:24-34）用 `policy:seed` 查 BTreeMap 取单个动作；`OBJECTIVE` 常量存在但无 AUC 计算；OOS/censored 合并成同一默认结果。 |
| `evo-core/src/curriculum.rs` | 缺口仍存在 | `LearnerState` 仅 checkpoint/failure_clusters（:8-11）；`next_task`（:51-63）在空 failure_clusters 时返回错误。 |
| 同上 `proposal_from_text` | 缺口仍存在 | curriculum.rs:65-76 文本校验后直接 `oracle_ok: true`。 |
| `evo-engine/src/curriculum.rs` | 缺口仍存在 | `step`（:6-14）先 reserve 再选题，选题错误归一为 `Error::NotFound`。 |
| `evo-engine/src/evaluator.rs` | 缺口仍存在 | `grade`（:80-103）对 `outputs_complete=false` 直接判 invalid，无计划内早停/连续前缀/终止证书路径。 |

v4 新增项（HoldoutManifest/ExposureLedger、normal-mixture 置信序列、EarlyStopCertificate、ExplorationCaps/ElasticPolicy/RecoveryEntry、ReplaySimulationProfile/ReplayReportV2、rsia.pareto_attainment.v2、PlateauSignal/CoverageProbeDecision、TestProposal/ValidityReport 独立 oracle）在 `crates/` 中无任何对应实现（文本检索确认 0 命中）。**结论：v4 相对 v3.3 的全部升级义务在本基线均为 planned，不因 v3.3 时代已有 PR 推定完成。**

### v4 E00 本地命令记录

输入 HEAD：`559d4da2f3c9ffd0e34a442bccd1d444214f69f5`（分支 implementation/host-cli-http-mcp-20260917，工作树已确认与 559d4da 内容一致）。toolchain：rustc 1.98.1 stable。

| 命令 | 退出码 | 日志 |
|---|---|---|
| `cargo fmt --all -- --check` | 0 | 基线本已格式化 |
| `cargo check --workspace --all-targets --locked` | 0 | `out/e00-v4/check.txt` |
| `cargo test --workspace --locked` | 0（126 项通过） | `out/e00-v4/test.txt` |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 | `out/e00-v4/clippy.txt` |
| `cargo build --locked -p rsia` | 0 | `out/e00-v4/build.txt` |
| `python3 scripts/smoke_workspace.py` | 0；migrations=0001/0002/0004，next_free=0003 | 0003 保持为空号，未被占用 |
| `python3 scripts/smoke_cli.py target/debug/rsia` | 0 | `SMOKE_CLI_OK` |
| `python3 scripts/smoke.py` | 1 | 脚本需要二进制参数；等价内容由 smoke_workspace.py + smoke_cli.py 覆盖，不伪造通过 |
| `python3 scripts/inventory_baseline_tests.py` | 0；112 项实际测试 | 全部 `legacy_equivalence=unverified`，无 T 编号，不编造 |

远程 CI：GitHub 账号 billing lock 状态本轮未复测，沿用历史 blocked 记录；后续 PR 合并以本地真实退出码 + 代码审阅为准并如实登记。

### v4 E00 范围裁决

- **唯一真源切换与 §18.2 归档（V071@E00）**：本轮已执行——台账入口切换为 v4 并登记内容摘要；v3.3 原件按原字节复制（非移动）到 `archive/2026-09-17-pre-v4/`，manifest 含原/新路径、SHA-256、原因与 superseded_by；旧归档目录保留不动。
- **历史包归并子项（V073）**：仍 blocked_not_found（原包与增量包均不存在），不重建、不推定。
- **可重建基线与 §1.4 复核（V001/V072）**：implemented_not_verified（PR 待开）。
- **WIP 处理**：`implementation/host-cli-http-mcp-20260917` 分支上的未提交改动（387 行 host CLI/HTTP/MCP 接线）已完整保存为 git stash；因其把 `evo_engine::service::HostService` 实际接入 HTTP/MCP/CLI（属 v4 产品面新交付），不与 E00 基线 PR 混合，待 E00 合并后按单一负责人审阅单独处理。

## E00 合并记录

| 字段 | 值 |
|---|---|
| plan_version | v3.3 |
| 方案正文 SHA-256 | `1b5870341be7c027c369a08c6e6bfd8f06dc494fc7174fff8930f42ae247294a` |
| source_sha（PR head） | `4b36f1681589c04de882f89252bf9d0e42b685f3` |
| PR | https://github.com/acosmi/RSIAgent/pull/3 |
| merged_sha | `5e40b0b5cba4e06cb89981d68be778107c292967` |
| 测试入口 | `cargo test --workspace --locked`；`python3 scripts/smoke_workspace.py`；`python3 scripts/smoke_cli.py` |
| 本地退出码 | fmt/check/test/clippy/build/smoke 均为 0 |
| 远程 CI | blocked：`The job was not started because your account is locked due to a billing issue.` |
| 支持范围 | 当前远程树可重建；增量包/原包未归并 |
| 回滚点 | 回退 PR 到 `6e83d1285b778a62243446bbe0a1eb14809de4f3`，不强推 |

## 环境与变更记录

- 本机 Command Line Tools git 可用；`/usr/bin/git` 在未同意 Xcode license 时失败，执行使用 `DEVELOPER_DIR=/Library/Developer/CommandLineTools`。
- 本机 `rustc`/`cargo` 1.98.1。付费模型凭据未在本轮配置；不得运行付费小试。
- 真实宿主未接入。E07/E16.4 在缺少宿主时登记 blocked，不降低验收。
- 物理归档：`archive/2026-09-17-pre-v3.3/`（gitignored）。Desktop 上 v3.2/v3.0 原件未移动。
- 远程只接收源码、测试、CI 与本台账索引。
