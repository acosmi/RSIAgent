# RSIAgent 实施台账

规范入口（本地，不进远程）：`RSIAgent-v3.3-工程实施方案-2026-09-17.md`。
plan_version：v3.3。本台账只索引该规范的任务与验收，不能独立修改门禁。

仓库：`acosmi/RSIAgent`。日期：2026-09-17 UTC。

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
| E00 | 归并真实源码与可重建输入 | implemented_not_verified | PR #3 squash-merged `5e40b0b5cba4e06cb89981d68be778107c292967`。本地门禁退出码 0。GitHub Actions 因账号 billing lock 未启动，远程 CI 记 blocked，不把未运行当通过。增量与原包仍 blocked。 |
| E01 | 冻结实验、任务分区和预算可行性 | implemented_not_verified | PR #5 merged `a62451a20e16b8f5cf58ca0200afb90676237058`。付费小试未授权。远程 CI billing-locked。 |
| E02 | 最小版本化契约与宿主能力边界 | implemented_not_verified | PR #6 merged `991d47efb35bd044c629df4548b9d97d943b1773`。远程 CI billing-locked。 |
| E03 | 跨任务证据接入生成消费者 | implemented_not_verified | PR #7 merged `fec2f32b3c4ff860fb845a08a3864a723c87b30a`。 |
| E04 | 可信执行、隔离与根资源预算 | implemented_not_verified | PR #8 merged `fb5f50e12437a817e7fe51f1098474589d22ce50`。 |
| E05 | 独立验收器与有边界统计判定 | implemented_not_verified | PR #9 merged `d2d32c63a82322738be562e0b47a2bd696a192f8`。 |
| E06 | 组合发布、实际应用与最小撤销闭环 | implemented_not_verified | PR #10 merged `18057b4fd81def2a76566c5d1f8c691f578f5b18`。 |
| E07 | 第一个最小可验证真实闭环 | implemented_not_verified | PR #11 merged `38b4f39eeee6ece7b28324d0d915eda0ceab9ab4`。真实模型仍 blocked。 |
| E08 | 可恢复的撤销、保留和备份链 | implemented_not_verified | PR #12 merged `cfc6a542cec4779ffce500c917d1fbf656d0e71e`。 |
| E09 | 生成/探索解耦与有状态在线探索 | implemented_not_verified | GenerationStrategy 与 ExplorationPolicy 分离；W=1；search_parent≠approved_parent；中间节点不发布。 |
| E10 | 不可变世界池与纯查表回放 | planned | 前置 E08,E09。迁移号在合并基线后分配，不重用 0003。 |
| E11 | 验证回放优化的真实经济收益 | planned | 前置 E05,E07,E10。 |
| E12 | 学习者条件化的经验自主获取 | planned | 前置 E07,E08,E09。 |
| E13 | 长期部署适应与能力保留监测 | planned | 前置 E07,E08。 |
| E14 | 受限改进器自身的继承控制器 | planned | 前置 E07,E09。 |
| E15 | 后继质量与跨代收益实验 | planned | 前置 E05,E07,E14。 |
| E16.1 | 来源导入与版本化读取器 | planned | 前置 E02,E03。 |
| E16.2 | 资产导入/分享与隐私门禁 | planned | 前置 E02,E06,E08。 |
| E16.3 | 内置种子与本地修改保护 | planned | 前置 E02,E06,E16.2。 |
| E16.4 | 额外真实宿主与配置面漂移 | planned | 前置 E02,E06,E07。无真实宿主则 blocked，不换成 mock。 |
| E16.5 | 持久恢复、容量、依赖与部署安全 | planned | 前置 E04,E08 及实际启用的 E16.1–E16.4。 |
| E16.6 | 发行、证据台账与唯一真源交接 | planned | 前置本次声明范围。 |
| E17 | 可选：开发代理评分器演化 | planned | 默认关闭。前置 E15。 |
| E18 | 可选：自动提出代码修改 | planned | 默认关闭。前置 E04,E06,E15。无隔离与人工批准保持 disabled。 |

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
| V001@E00 | implemented_not_verified | 输入 HEAD `6e83d12`；先记录再跑。本地：fmt 0、check 0、test 0（15+5）、clippy 0、build 0。CI 待跑。 |
| V071@E00 | implemented_not_verified | 本地入口 v3.3 SHA-256 `1b587034…e247294a`；`archive/2026-09-17-pre-v3.3/` 只读归档；`.gitignore` 阻止方案与 archive 进远程。 |
| V072@E00 | implemented_not_verified | 清点 20 项（evo-core 15、evo-storage 5）。无 T 编号。`legacy_equivalence_unverified`。 |
| V073@E00 | blocked | 原包与增量包缺失，不认证来源、不默认复制、不从方案复造。独立实现可继续。 |
| V080@E00 | implemented_not_verified | 映射写入本台账；计划/来源/静态测试 ≠ 已实现/已运行。 |
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
