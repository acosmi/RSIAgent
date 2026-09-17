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
| E01 | 冻结实验、任务分区和预算可行性 | planned | 前置 E00。V010–V013。无资金授权时只准备脚本。 |
| E02 | 最小版本化契约与宿主能力边界 | planned | 前置 E00,E01。V002/V003/V005/V009/V043–V048/V060/V062/V078/V079。 |
| E03 | 跨任务证据接入生成消费者 | planned | 前置 E02。V004–V006/V017/V051/V052/V054–V058。 |
| E04 | 可信执行、隔离与根资源预算 | planned | 前置 E02。V007/V008/V009/V038。 |
| E05 | 独立验收器与有边界统计判定 | planned | 前置 E01,E03,E04。V010–V013/V028。 |
| E06 | 组合发布、实际应用与最小撤销闭环 | planned | 前置 E02,E05。V014–V017 及 V047/V049/V050/V059/V064/V070/V075/V077/V078。 |
| E07 | 第一个最小可验证真实闭环 | planned | 前置 E03–E06。真实链；mock 不得冒充模型能力。 |
| E08 | 可恢复的撤销、保留和备份链 | planned | 前置 E06。 |
| E09 | 生成/探索解耦与有状态在线探索 | planned | 前置 E03,E04,E07。 |
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
