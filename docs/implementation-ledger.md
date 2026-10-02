# RSIAgent 实施台账

唯一规范入口（方案正文仅本地）：`RSIAgent-v4.2定稿-工程实施方案-2026-09-30.md`。
plan_version：`v4.2`；plan_sha256：`70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455`。谱系：v4.1 `RSIAgent-v4.1定稿-工程实施方案-2026-09-19.md`（SHA-256 `45f3ba068b988cc502a96c15bd737e1688084dd21de33c2dee633f484531e150`，2026-09-30 归档至本地 `archive/2026-09-30-pre-v4.2/`）→ v4.2。历史验证记录保留其验证时的 plan_sha256（真源 §18.8）。
本台账是实施与证据索引，不另立规范；历史 PR、测试和旧台账不决定当前规则。
用户于 2026-09-19 明确确认 v4.1 替代 v4，并授权将本台账上传远程；方案正文、内部合同、归档及 QA 原始材料只保留本地。2026-10-01 第四轮由 Codex 主控持续推进；实施子代理统一使用 gpt-6.1-sol，逐卡记录思考档位。每个任务独立 PR；主控全量复跑及至少两个反例通过后，以固定 head 按依赖顺序合并并核对合并树。历史轮次的合并操作者按各节原始记录保留。Actions派发、付费运行和部署/发布未授权。

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

### 第四轮本批事件逐项折入

#### 2026-10-01T15:38:27.374930+00:00 AG067登记探针设置纠正
- 首复制registration脚本替换匹配list但原文是set，仍查AG063旧记录而exit0；原日志registration-setup-old-records.log保留，不作为新登记通过。纠正到三新id后重新运行实际基线。
- AG065两个crate新测试同basename，为明确primary日志匹配采用core16作为索引入口，engine18作为additional_test_sources；两者均在原独立全量，未重写日志或范围。原本地输入保留*-pre-ag067.json，索引尚未登记该条。

#### 2026-10-01T15:48:17.923361+00:00 压缩续跑闸门与AG066/#111创建
- 提示词b0000207…与真源70ec06e4…匹配，完整重读提示词与规定材料，截断段补读；auth/fetch0、cargo1.98.1/Python3.12.14、磁盘34GiB。未改凭据。
- AG066同一精确push新增可信事实审批获准并成功，draft#111 https://github.com/acosmi/RSIAgent/pull/111 已创建及附chat，fd4a1aa1620b73a2df1f8c567bfd0770ece2777c，尚未主控验收。AG067三文件完整diff已审，机械fold与64条旧记录保持核对，precommit结构通过；先完成台账再叠放066。

#### 2026-10-01T15:52:00.705678+00:00 AG067/#112 全量启动与AG068派发
- AG067 draft https://github.com/acosmi/RSIAgent/pull/112 已创建附chat，fixed a18cedd50ce5e65bd090f0e17d7a261c3e0e1698 父c6e13e906ea888916294e6deac77c37f4391225f，三文件+277/-12完整读审。fold SHA6378304cbb298b5045f94c1552862cbf3210e6812aff5e14cc549d0778357fa4、64旧记录相等，precommit checker通过；全新target-ctrl-ag067独立八门进行中，不改该WT。
- AG068 E08/§11、V017/V018/V038/V075限定已登记native authority后继，新卡cards/AG-068.md SHA256c56eb468495b05a4d9e16b92d7eb3845542cda180b288f0e771a8264de851ff0；真源§11L1077/§11.3L1110–1112/§11.5L1132–1134/E08L1295,L1299，精确typed分类，不新建生成链/不改旧Failed。基线c6e13e9，WT ag-068，branchwrokbot/ag-068-trace-successor-cleanup，复用/root/impl_ag065，gpt-6.1-sol/max（安全恢复）；两文件白名单，与066单脚本/067台账不重叠，实施1/root full共2路构建。
- SC-GB16-R3完整报告已归cards/SC-GB16-R3-r4-report.md SHA25618651edec4702db9a309f3e852f56a6ff696410cff1f0a1f1cccc03cf33874e5并全读：正常Deepen已展开父在线/回放差异静态可复现，终态/complete_support合同缺定义先挂起，不凭旧测试作为规范。
- 用户显式增加线程目标，并补充严格第一真源；已登记active目标，不设未请求token预算。

#### 2026-10-01T15:54:09.672726+00:00 CTRL-AG067-R1 / #112 独立验收通过
- 固定heada18cedd50ce5e65bd090f0e17d7a261c3e0e1698，实际父c6e13e906ea888916294e6deac77c37f4391225f，三白名单文件+277/-12全部独立读审，无叠放/冲突。fold SHA2566378304cbb298b5045f94c1552862cbf3210e6812aff5e14cc549d0778357fa4仅标题层级变化，64旧verified记录不变。
- fresh target-ctrl-ag067，qa/ctrl-ag067-a18cedd.log*八门exit0；1263/0=父1263+0，90测试二进制+5doc组；Python48，定向ctrl-ag067-support-scope-a18cedd.log14/0。
- root Python probe-ag067/probe.py SHA2564444570ada20a2176b34634d082913c04985b8714dcd281bf49454f5408b37e7；head ctrl-ag067-adversarial-head-a18cedd.log全通过，实际父ctrl-ag067-adversarial-base-c6e13e9.log因三新记录缺失exit1，旧篡改/正向/clean通过。三条source/merge第二父/tree/祖先/blob/input及全部日志hash、全量8退出码/计数/额外engine18与core4 blob逐项核对；5篡改分别旧AG042输入、新AG062输入、065跨scope日志替换、064错误merge、064新输入均拒。
- AG065两crate同basename，primary明确core16，engine18额外绑定；输入变更前清单*-pre-ag067.json留存。原始日志不改。初registration替换误匹配set导致查旧记录的exit0保留于setup-old-records，不作成功；纠正后真实parent缺三记录exit1。
- verified仅E00/E16.6：E00L1181–1189/E16.6L1459–1463/§18.5L2162–2166，V001/V071/V072/V073/V074/V080/V098三项已验子范围追溯。062不证明P/B真实性，065不证明类型/默认/真实消费者，064不证明完整合法集/修复生产链/旧报告迁移；066未验，V076完整链未验。
- 不声明新产品/完整E-V/历史归并/真实效果/发行，历史部分input缺失保持。回滚参考父c6e13e906ea888916294e6deac77c37f4391225f，保留证据/费用/撤销。按启动§5索引任务采用Python反例；产品返修0，主控输入适配已披露，审批拒绝无。合并采用fail-fast正文读回/head/当前父/clean，admin仅保护需要时。

#### 2026-10-01T15:56:15.253168+00:00 合并AG067 / #112
- fixed a18cedd50ce5e65bd090f0e17d7a261c3e0e1698，merged_shae7ec4904b42e8c6f0d20e425b3bdc77c3109c180，mergedAt2026-10-01T15:54:38Z，双方treef0c1d8c2b6cfc7a71a2c8e9c9684cf1f68faf7a9相等，实际两父base/head相等。fail-fast正文/readback/fetch/head/clean全部成功，admin=false，main已ff-only。台账周期归0，清理随后，QA保留。

#### 2026-10-01T15:56:15.253168+00:00 用户追问完整链与旧Failed恢复后续
- 主控说明V076不得用手写Active/伪造可信执行代替实际消费者；承诺继续拆解可实施段。旧Failed恢复此前直接归待合同过于保守，重新核查现有接口可裁决范围；queue原条作为历史判断保留，当前以本条为准。
- SC-FAILED-CLEANUP卡SHA25626518b81c34a74249399d23e69209b4315d43bc264edde8b3d36fd5e0fb6b1fd，派用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/max。只读当前main，核真实Failed路径/游标/既有Admin入口/可实施最小设计，禁构建与写入；AG068同文件修复完成后再派后续，不能并行改。

#### 2026-10-01T15:57:46.206064+00:00 SC-IMPORT-CHAIN只读派发
- /root/impl_ag057，gpt-6.1-sol/max，卡cards/SC-IMPORT-CHAIN-r4.md SHA25604861ec057cf1094efb6ced863b9619a26ef55f17eef01eb226037b2b5d6dacb。真源§6.3–6.7/§11.2L1100,L1106/E16.1L1421/V076；仅主main逐段实核保持未验证身份的合法开发材料路径，找可实施最小闭环和真实阻塞，不把现代码guard当规范。不写不构建，不触AG068WT。
- #112清理两WT/branch-d/remote单独delete/专属target均完成，QA保留。

#### 2026-10-01T15:58:31.960897+00:00 AG066/#111叠放与独立全量
- 单提交fd4a1aa1620b73a2df1f8c567bfd0770ece2777c从6f0e5fa35b582f0f4cde4207ddf59d65a51b180a叠至e7ec4904b42e8c6f0d20e425b3bdc77c3109c180→1d0d93d1dc42deae83056bfc8602fd3bf2760f3d。唯一script补丁before/after-rebase.patch逐字节cmp0，无冲突；literal full旧head/fullref lease成功。
- fresh target-ctrl-ag066跑ctrl-ag066-1d0d93d.log*，预期1263/0+47额外Python。实际父base-e7ec490-ag066/target-ctrl-base-ag066仅构建rsia供同冻结root双探针；与AG068最多3路编译，full期间不写066WT。

- 2026-10-01T15:59:03.307524+00:00 AG066栈位正文首调用在读尚未生成的本地body文件时exit1，未发送API；已先生成实际body再提交，未影响代码/验收。

#### 2026-10-01T16:00:53.514605+00:00 AG066主控探针合法夹具纠正
- 首次父双探针2失败，原因root把非ASCII/斜杠/引号放入request_key，现有identifier只许ASCII字母数字及._-:；合法payload前提不成立，非产品缺陷。原脚本及日志保留*-invalid-key-setup。只改root夹具为合法ASCII key，UTF8/转义比较移到允许的goal/details/content；两树使用同一修正版，不改产品/harness。

#### 2026-10-01T16:08:15.721686+00:00 压缩闸门与AG066全量完成
- 提示词b0000207…及真源70ec06e4…、v4.1归档45f3ba06…匹配；完整重读启动提示词/规定材料。归档首次手写09-29文件名不存在，rg定位09-19后hash匹配；gh auth首次瞬时invalid而fetch0，原请求复核auth0，未改凭据。cargo1.98.1、Python3.12.14、27GiB。
- 066full八门退出均0、1263/0、95结果行含90bin+5doc，48Python；root额外验收wrapper启动。修正rootprobe SHA2565bc88fb8e089b42f4e83d68d02e249d5f2298b68fed8abac43a25a7b8b07a91a，实际父两个边界2/0，原错误夹具证据保留。

#### 2026-10-01T16:10:44.720657+00:00 CTRL-AG066-R1 / #111独立验收通过
- 固定1d0d93d1dc42deae83056bfc8602fd3bf2760f3d，父e7ec4904b42e8c6f0d20e425b3bdc77c3109c180；原fd4a1aa1620b73a2df1f8c567bfd0770ece2777c从6f0e5fa叠放，唯一patch cmp0。唯一scripts/test_mcp_stdio.py +636/-0，全文已读，SHA2565663ebb36a611087a08969c9d95eb58bc5ab32ec90c1a0ed430d2a62545541e9，WT与父树clean。
- 全新target-ctrl-ag066：ctrl-ag066-1d0d93d.log*八门exit0、1263/0=父1263+0，90bin+5doc，检查器48。额外真实disabled stdio ordinary/-O reverse/restored各47/0 exit0，各54正常EOF+7预期帧拒绝exit1，无强制退出。原实施基线1215/0；初sandbox1210/5环境失败保留，R1重跑通过。
- root两自写边界，冻结source-test-mcp-stdio-5663ebb.py同SHA，probe.py SHA2565bc88fb8e089b42f4e83d68d02e249d5f2298b68fed8abac43a25a7b8b07a91a。head ctrl-ag066-adversarial-head-1d0d93d.log与实际父ctrl-ag066-adversarial-base-e7ec490.log均2/0 exit0。P1真实UTF8/转义JSON、key顺序、空白/CRLF在三写工具返回同完整幂等结果且原对象不变；P2非法goal不占key，经两次EOF/restart后合法首次成功、冲突不改原对象、成功缓存前仍做业务验证。测试覆盖PR不改产品，两边均通过是边界核验，不冒称产品修复红灯。
- root mutate_responses.py SHA256f17d8abf18adaf76d8d29f046c709d7c254184a32d24d82c4f64e1337784a513，真实进程解码后仅改响应。精确错误码invalid_input→forbidden且双通道一致：2hits，45/2，exit1；合法text与structured verification不同：30hits，17/30，exit1；无errors，均正常断言失败；恢复47/0。初root probe把非法Unicode/斜杠放request_key导致父0/2为设置错误，原probe-invalid-key-setup.py/日志保留，修正版将Unicode放合法goal/details/content，两树一致。正文首修改本地文件尚不存在exit1未API，已纠正。
- verified仅E02/E16.4 §6.1L605/L609、E16.4L1443–1445、V002四工具旧v1/本地身份/重连/错误和严格帧子范围；V039只声明本机fixed rmcp3.3.0、rsia0.2.0、协议2025-06-18实测边界。精确四golden与2000字节预算，47矩阵覆盖，不更改生产/CI/锁。
- 不声明真实Claude/其他宿主/跨平台/业务取消/v2协商/全表零写/真实消费者/计费安全/质量收益/可发行。stderr临时文件不等于绝对磁盘上限。回滚参考父e7ec490，纯删除新脚本，保留QA/费用/撤销事实。
- 卡AG-066.md SHA2560e995b9ad068fc5d99f2ffab51105babd1262b0bd2f3fd94458567f1d448219c；R1环境b56f9ddefe9863edf7e3f89cfe03a52b2fd45c70bccef3ef5241dc8009a49824。产品返修0/环境裁决1，无冲突。实施push自动审批拒绝原文保留；root实际payload/remote public admin权限/直接人类指定hash授权新事实同请求复核获准，未绕过。完整report bbae556312f720436980cba6d36069615ecbdcaab52d28957269161f6a5006c7。

#### 2026-10-01T16:13:08.194114+00:00 合并AG066 / #111
- PR #111 fixed1d0d93d1dc42deae83056bfc8602fd3bf2760f3d，merge200c43bc19368ff52b064e67eb1b14e35f096d39，at2026-10-01T16:11:27Z；accepted/merge tree53426fe775001c01f8e09437c4a3d4e4b1faf80c相等，两父basee7ec490+head精确相等。admin=false，main已ff-only。首次body PATCH EOF保留，同请求重试后全文读回通过，全部硬前置成功再merge。清理随后，QA保留；台账周期产品1。

#### 2026-10-01T16:13:08.194114+00:00 SC-FAILED-CLEANUP裁决
- 完整报告归cards/SC-FAILED-CLEANUP-r4-report.md SHA25640a5846380e02aaae62e51da83462acf8f18386ea9d4a6772b3d7303df3b3e48。root已读全部，独立核cleanup_step/expand_frontier/cleanup_node_content/受控export目录/0006结构；确认Failed仍推进部分frontier，节点expanded先于内容分类，state切换不能续清，export失败可已脱敏仍残留目录。选现有Admin显式全frontier重扫+副作用再验，不需新接口/迁移/合同。先AG068，再独立AG069，保留原未知Failed/可信anchor边界。

#### 2026-10-01T16:15:48.745629+00:00 AG069草案及安全复核
- 主控按真源L1077/1112/1132–1134/E08L1299写AG-069-draft.md SHA2568ecfe24c11d26e29aedad3bc67f69d1044bd7213431e3e5c96f7040bfcd3d6b6；未派实施，等068同文件串行。原辅助chat gpt-6.1-sol/max只读R2复核完整重扫/旧export事件副作用/多失败/Worker/计数，不改代码或新权限。AG066已完成全部正常清理，日志保留，磁盘29GiB。

#### 2026-10-01T16:23:26.609497+00:00 AG068主控双反例与AG069真实旧库
- root冻结AG068源夹具source-import-source-successor-pre-review.rs SHA256f36a3c5b808c68d7e74c628475bd796a4b412ebe83443227db96c37b2f0b6329，append.rs cae73ba08e84497a0719decf81195a0248a3a52c1edc68bdce07e422b5b63176。实际父200c43b独立target编译成功，两反例0/2 exit101，日志ctrl-ag068-adversarial-base-200c43b.log；P1双水位/双job共享authority交错分页，P2cycle及expanded后迟到native。父原生authority都被unknown_scope拒；head尚未交付，未在实施WT运行。
- 相同实际旧树200c43b用root独立generate-old-failed-ag069.rs SHA2569d092f2fdf6790bc9b7bec14ca734196b974ca375987d993400830b395ad413b真实Host writer造3authority+显式typed边→begin_revoke/cleanup_step(page1)→Failed，processed8/pending0，3块原文仍在且缓存已redacted。Store.backup一致性副本qa/old-failed-ag069/rsia.sqlite3 SHA2564fdb653a0f78fbefa3ef813522564a6f09e929736e95df2fe069b7ef04228b4a，controller-provenance.json和backup-manifest完整保留；生成测试1/0 exit0，日志ctrl-ag069-old-failed-generate-200c43b.log。无SQL伪造state，无真实模型/账单。用于069升级回归，原产物不修改不上传。
- SC-IMPORT-CHAIN全部决策要点与事实主控存档cards/SC-IMPORT-CHAIN-r4-report.md SHA256b6b60e0784067d18c3c197e22f3ab6d4afa3d06603deb40e5fef27aa60c76b70，明确摘要存档非原文。主控重读真源L629/650/658/684/715–765，G1可信入口不放宽，E16独立材料入口有依据。R2 /root/impl_ag057 gpt-6.1-sol/max只读精化AG070真实typed材料读取最小接口；不以fixture并行生成器替代Broker/Runner接线，后续必做登记。

#### 2026-10-01T16:23:43.077408+00:00 用户休息期间继续授权
- 用户明确“我去休息了，要持续推进，不要随意停止，严格按照第一真源持续推荐”。按上下文为持续推进的重申，不是收尾；继续已授权主控/子代理/固定head合并流程，不因用户暂离等待确认。真正待用户/外部阻塞单项登记，其余任务继续。

#### 2026-10-01T16:25:32.887708+00:00 AG068环境R1
- workspace5个已有HTTP localhost EPERM由root独立读日志核实。AG-068-R1.md SHA256e1d2aeb90ef74cdb0b52632d6dba1d10ab75d2a51bf32bf047d57b1482ad5bff，仅允许同命令/同target向工具申请执行环境复跑，原失败保留，不改产品/断言，产品返修0。已发原agent max。

#### 2026-10-01T16:26:39.566010+00:00 SC-PRIVACY-UTF8只读侦察
- /root/impl_ag054 gpt-6.1-sol/max，cards/SC-PRIVACY-UTF8-r4.md SHA256af5e6e765baa7e35b3f09a7be6e806223726f229e1758d35cffe18e0ca48bcc1。主控读packages.rs发现既有privacy snippet按byte切片可能panic，作为后续导入安全投影前置，仅核只读事实/最小修复，不改变扫描policy，非绝对安全证明。实际基线200c43b；不构建、不读他WT。

#### 2026-10-01T16:28:17.388378+00:00 AG069草案R2纠正
- 完整报告SC-FAILED-CLEANUP-R2-r4-report.md SHA25616979d84d1e7105a75316462fe5760e1cdbb5b2b92c1e7fbce3c10b7687ca5c0；主控采纳跨job共享export事件/absent检查/两cursor/last_error/Worker/SQL成本边界。草案SHA25684176d2e9b74dc436839733a98a86ee1d35216b912c47d4007192be15c6c472a；尚未派实施，同文件等068。privacy scout短暂中断以保持最多两只读侦察，R2完成后已恢复；无实现或构建被中断。

#### 2026-10-01T16:33:36.342075+00:00 AG066下一台账输入清单
- qa/inputs/ctrl-input-ag-066-1d0d93d.json SHA2569f982bc0afa115e267537cb1779beef852d788b7301cd263da811509e2385ff8。为符合既有Rust命令文法，primary明确已有evo-mcp service 2项回归，区别同名HTTP service5项；实质新增Python47项以additional_test_sources、三次成功日志和两变异日志逐项hash绑定。Rust单条命令不代表复现Python47，scope_note明确，不伪造命令或计数。下次台账登记须读此完整范围。

#### 2026-10-01T16:44:09.198394+00:00 续跑闸门与AG068外发复核
- 提示词b0000207…、真源70ec06e4…、v4.1归档45f3ba06…匹配；auth/fetch0，cargo1.98.1、Python3.12.14、26GiB，main200c43b clean。用户问真源路径已明确回答，未收尾。
- AG068 fixed1ef06f10e2ffb1869cbf714df5c381b28963596d，2白名单文件+884/-0；先前完整逐行审阅后本轮重核字节SHA一致。最终交付及原拒绝归qa/impl-ag068。子代理在process前被auto-review拒绝push，零外发；root没有更换通道。
- 新可信事实：本chat直接人类明确要求执行指定hash提示词，§3.4明指acosmi/RSIAgent产品draft与§10E16持续队列；实际载荷仅47行分类及837行合成测试，无真源/out/archive/凭据。root只读gh API再次核owneracosmi/public/admin-push，origin一致；向工具申请同一精确push复核。

#### 2026-10-01T16:44:09.198394+00:00 派发AG071
- E16.2/§11.2，V067限定UTF8扫描不panic；正式卡cards/AG-071.md SHA256e20c8b42c67a88fbc9072491947da2b82396b9e3a445b288d89cdde5512c56a9，依据真源L1104/L1427/L1429/L1575。
- 基线200c43bc19368ff52b064e67eb1b14e35f096d39；WT ag-071，branchwrokbot/ag-071-privacy-utf8-snippets，target-ag-071；实施/root/impl_ag054，gpt-6.1-sol/max（安全拒绝）。packages局部及新测试共2文件，与AG068不重叠。先定向，full前root协调磁盘；实施1+root验收1。

#### 2026-10-01T16:47:04.069683+00:00 AG068/#113叠放与主控全量
- 原精确push凭新可信事实复核获准成功；draft https://github.com/acosmi/RSIAgent/pull/113 已创建并附chat。1ef06f10e2ffb1869cbf714df5c381b28963596d从c6e13e9叠至200c43bc19368ff52b064e67eb1b14e35f096d39→75754911a01aab756d5f17b9d6fef8bb3959de33，单提交before/after patch逐字节cmp0，完整字面旧head/fullref lease成功，无冲突。
- fresh target-ctrl-ag068全量启动，ctrl-ag068-7575491.log*，预计1263+10=1273/0；期间不放探针文件。父200c43b双probe已0/2实际红灯，head待full结束。

#### 2026-10-01T16:49:58.780095+00:00 AG070材料读取正式裁决与派发
- SC-IMPORT-CHAIN R2报告保真要点cards/SC-IMPORT-CHAIN-R2-r4-report.md，主控独立读真源L629–684及importexecute/load/event/核心coverage和aggregate，形成4文件卡AG-070.md SHA2562dbd6904f5271a4a1d5c8ea16c27398afac8f7e56d7241a8ebff07b0e35e9127。E16.1/V005/V006/V017/V051–56/V076仅真实材料读取段。
- 基线200c43bc19368ff52b064e67eb1b14e35f096d39；WT ag-070，branchwrokbot/ag-070-import-development-material，target-ag-070；/root/impl_ag057，gpt-6.1-sol/max。与071packages及068lifecycle互不重叠，实施2/rootfull1，最多3Cargo。full前协调磁盘。新API缺编译不是基线缺陷，先旧公共API实际事实；NoModelDispatch/Unreviewed与G1门保持，不整体挂起完整学习链。

#### 2026-10-01T16:53:27.108900+00:00 CTRL-AG068-R1 / #113独立验收通过
- 固定head75754911a01aab756d5f17b9d6fef8bb3959de33，实际父200c43bc19368ff52b064e67eb1b14e35f096d39；原1ef06f10e2ffb1869cbf714df5c381b28963596d从c6e13e9叠放，单提交before/after-rebase.patch逐字节cmp0，literal fullref/oldhead lease成功，无冲突。2白名单文件+884/-0已全文审阅并重核SHA一致；产品47行严格wire分类，其余837行10测试，现有测试未动。
- fresh target-ctrl-ag068，ctrl-ag068-7575491.log*八项exit0，1273/0=父1263+10，91测试二进制+5doc，Python48。实施报告qa/impl-ag068/handoff-report.md SHA25676a4a9f6736c009075f93bb84e2cb01dba07f449e5b22540506f5a4c4d097c23，原baseline3/7、移分类3/7、schema-only8/2均成功编译后失败；27项负表schema-only25错误放行被抓。初workspace1268/5与CLI exit1环境失败保留，R1相同命令复跑1273/0/双smoke0。
- root冻结source-import-source-successor-pre-review.rs SHA256f36a3c5b808c68d7e74c628475bd796a4b412ebe83443227db96c37b2f0b6329；append.rs SHA256cae73ba08e84497a0719decf81195a0248a3a52c1edc68bdce07e422b5b63176。head ctrl-ag068-adversarial-head-7575491.log2/0 exit0；实际父ctrl-ag068-adversarial-base-200c43b.log0/2 exit101，正常编译且两树clean。P1双来源水位/双job交错分页共享native，只脱敏一次且original_digest不重哈；P2环与expanded后迟到native由fixpoint收齐，重复Complete无变化。手工typed边含故意绕写门明确是清理夹具，不冒称生产学习链。
- 额外root旧库边界old-failed-upgrade-boundary.rs SHA256c71aeb2973b285039112180950e6621f9ebde94d2415225e62814fe1d67f44b5，ctrl-ag069-old-failed-control-7575491.log1/0。实际200c43b旧程序Host+begin/cleanup生成backup DB SHA4fdb653a0f78fbefa3ef813522564a6f09e929736e95df2fe069b7ef04228b4a复制后在本head三次Admin/reopen仍Failed/pending0/三native未清；原件逐字不变。此为AG069真实升级父对照，不能算068修复旧Failed。
- verified仅E08/§11 L1069–1081特别L1077、§11.3L1110–1112、§11.5L1132–1134、E08L1295/L1299；V017/V018/V038/V075仅已登记闭包中新Pending/Running作业的合法native authority清理、分页重开、非primary来源与共享/环/迟到边。外层与record strict，信任/用途/identity/family/hash/excerpt绑定，diagnosis不新增Host原本没有的语义门；5outcome合法均清。未知仍Failed、直接源run删除不变，原id/digest/audit/edges/cache墓碑保留。
- 不声明完整V076/生产import→native接线、可信升级、所有Pending读闸、generic run gate、旧Failed恢复、任意未含新edges旧备份保护、物理擦除/完整E-V/效果/发行。旧Failed下一AG069串行，依据真源可实施，已撤销整体待合同判断。回滚参考实际父200c43b，保留水位/费用/审计/已脱敏事实，不还原明文。
- 卡AG-068.md c56eb468495b05a4d9e16b92d7eb3845542cda180b288f0e771a8264de851ff0，R1环境e1d2aeb90ef74cdb0b52632d6dba1d10ab75d2a51bf32bf047d57b1482ad5bff，产品返修0/环境裁决1，无冲突。子代理auto-review拒绝原文保留，root直接人类指定hash授权/actualpayload/public remote admin-push新事实同请求复核获准后建draft#113，未绕过。

#### 2026-10-01T16:54:52.510574+00:00 合并AG068 / #113
- PR #113固定75754911a01aab756d5f17b9d6fef8bb3959de33，merged_sha212c78488b403397eaebaa65a6d280b9c6be3899，mergedAt2026-10-01T16:54:00Z；accepted/merged tree9db6431801f1989be75792735aa7f8a7eca2b0e5相等，两父200c43bc19368ff52b064e67eb1b14e35f096d39与验收head相等。fail-fast正文读回/head/base/clean全部成功，admin=false，无冲突，main已ff-only。清理随后，QA保留；#112后产品2。

#### 2026-10-01T16:55:56.461648+00:00 AG068清理与AG069正式派发
- #113 clean/祖先核对后正常移除ag068与base200c43b、branch-d/remote逐个delete完成；三专属targets移除，QA/旧DB保留。
- AG069 E08/E16.5/§11 V017/V018/V038，卡cards/AG-069.md SHA256b988de7f2245c7b495f5fa013eb930c0b48505d14de45725feb1dddf67bd5c30。基线212c78488b403397eaebaa65a6d280b9c6be3899；WT ag-069，branchwrokbot/ag-069-failed-cleanup-resume，target-ag-069；/root/impl_ag065，gpt-6.1-sol/max（安全恢复）。3新/改白名单及068单个既有测试授权，旧库只读复制范围明确；与070/071不重叠，实施最多3/编译最多3。full前root协调。

#### 2026-10-01T16:57:28.088186+00:00 SC-IMPORT-BROKER只读派发
- 卡cards/SC-IMPORT-BROKER-r4.md SHA2566870c00179739b4a223d9ab88f32cf0127a1c46aad097b26b43e13a880a9d1d7，用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/max（来源/计费/安全合同）。只读main212c784及AG070卡，不入实施WT，不构建/写文件。实核typed artifact broker/预算绑定、安全投影与独立生成后续最小API，不以当前缺代码整体待外部。当前3实施/1只读/最多3Cargo。
- AG071实施方基线成功编译后2/8 exit101，三类UTF8 panic含实际export/import/stage，ASCII38 findings及Clean/Warning正常；主控尚未验收。

#### 2026-10-01T17:00:05.553842+00:00 AG068下一台账输入及071主控反例准备
- qa/inputs/ctrl-input-ag-068-7575491.json SHA256d2a606dbae7cf15e2b3e7432215ac599af4fd98ca0d08d670421d944baade678，primary新engine10测试与完整源码tree/blob/八门日志/双probe/真实旧Failed对照绑定。下一台账#112后产品2，满3–5触发。
- root071 probe源packages_v41固定212c784，append SHA256cadb9c5c3580572d920cbe995717ec0c2410145146063bb47ebb437b997de6c6，分别从导出metadata/dependency/source-id及序列化foreign claim字段验证UTF8安全拒绝，实施测试聚焦member正文；实际父base-212c784-ag071已建未编译，等待Cargo空槽。一次静态读不存在core/packages.rs路径已纠正为engine/packages.rs，未据不存在文件下结论。

#### 2026-10-01T17:01:01.955738+00:00 AG070 R1字节快照裁决
- cards/AG-070-R1.md SHA25693acd38dbd2cbebc376735a837346dae909470b0ce89932aeb3fd47067ef484f；root读read_blob固定Vec/hash/身份/读锁，原卡把外部文件在复制后变化也要求单读检测过强，纠正为可证明快照绑定及返回前DB活性；不绕取root、不新storage API/重复全读。来源/闭包/用途/全部对象fingerprints与前后活性不收窄，明确现旧validate_live函数不是完整upstream证明。未来consumer须fresh读取。
- AG070 baseline公共API实际2/0，三reader/LocalPrivate/原片段/summary/撤销事实，不把新增API缺编译当缺陷；071新增测试首轮C:/Users/同时匹配/Users/导致预期错，原日志保留，授权范围内修新断言并重跑最终基线，产品policy不变。

#### 2026-10-01T17:05:05.665684+00:00 AG069主控反例准备
- probe-ag069/source-native-cleanup-f36a3c5b.rs与068冻结helper逐字相同；append.rs SHA256c5b72f7755ea3e6172856c6d3ae2d9c69aac904c77c7622c2c876c1a46634159。P1真实旧DB在另一个job抬高水位后Admin一次启动、Worker每页重启续清，原身份/水位/digest/history保留；P2旧恢复轮中在已expanded父之后插入两个真正Host写出后故意损坏的late fixture，先只修最后错误不能Complete，再全修才完成。尚未编译/运行，不能算通过。base-212c784-ag069已建，等待Cargo空槽。

#### 2026-10-01T17:07:02.868856+00:00 AG071主控探针设置纠正
- 父probe首次exit101是root错误调用未实现Default的ForeignMetadata，属于编译设置错误，不是产品红灯；原append与完整日志保留*-invalid-default-setup。已按实读公开字段显式构造，仅改root夹具，修正版append SHA256b136b3ba853541286710b15713b23cb1f77785351820bed1c02d38dea95a3b05，两树须使用此同一冻结版本。随后重新实际父对照。

#### 2026-10-01T17:08:22.614829+00:00 AG071实际父反例完成与全量空间协调
- 修正版rootappend b136b3ba853541286710b15713b23cb1f77785351820bed1c02d38dea95a3b05在实际父212c784正常编译，ctrl-ag071-adversarial-base-212c784.log0/2 exit101，15 metadata/foreignclaim情形实际panic；clean。编译设置初错单独保留不算产品失败。
- 实施071最终基线2/8含504边界中184实际panic，head10/0，两变异101恢复cmp0；root完整预读products+12/-6/new676行，SHA91ab1d48353d79ceedfcba5a02fea054964b35a530370823e63892f9336b3dc8 / 4f81e3104cbfc1d18ac4f1504c60ac30bcf3b0188f08bb9a221c47f148122103，尚未全量/验收。root槽归还，先既有回归/fmt/clippy，workspace前协调df。
- 当前23GiB；已验收任务target均清空，余056权限挂起target2.4GiB与在途/父probe各约0.6GiB；原始QA/卡/源码不清理。

#### 2026-10-01T17:08:54.544177+00:00 构建缓存空间裁决
- 23GiB且已验收targets已全清；为保持20GiB安全余量，移除长期权限挂起AG056的可重建target-ag-056（2.4GiB），其实施已结束/无运行Cargo。删除前核localhead66c67457cfa9e1ba80e3d2dc3ceb9562f9480cf9与clean，WT/唯一ci源码提交/所有日志/报告/分支保持；不改凭据。缓存提前清理明确记录，AG056仍未验收未合并，之后获权限也必须最新main重验，旧构建不得复用。未用cargo clean、不改Cargo.lock。

#### 2026-10-01T17:18:37.297510+00:00 续跑闸门与导入链后续侦察
- 提示词 b0000207…、真源 70ec06e4…、v4.1归档 45f3ba06… 均匹配；auth/fetch均0，cargo1.98.1、Python3.12.14、df22GiB。用户真源路径问题已回答，无收尾指示。重读规定材料及相关真源条款，main212c784保持。
- SC-IMPORT-BROKER-r4-report.md 收束报告完整原文归档并实读；明确旧broker来源id无kind、旧预算按run、缓存先返回、typed新ref必须同步恢复保护；AG070材料读取不是出站许可，registered runner未消费Skill不能证明效果。后续按安全投影→typed预算/broker→有界生成→Candidate/独立执行拆卡，代码缺口不整体挂外部。
- AG071仍self全量在跑；AG070新定向12/0（首轮9/2为fixture触发SQLite id CHECK，保留）；AG069定向baseline编译在跑。三Cargo封顶且22GiB，不启动root第四路；070全量等待协调。

#### 2026-10-01T17:20:12.917838+00:00 SC-BROKER-CACHE只读派发
- 用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/max，卡7c3af5468c3d259b22a33abfd1e5bb5933b966e7096fec3b7904b948cd7d52b9；main212c784。§11即时读门/V017–18，限定旧Completed缓存撤销后、不cleanup窗口的最小合同/夹具；90行，只读无构建。先前完整broker报告SHA6f8a7a5621e525a2342d4ba4a511e97fcb9f12f54c5967486ffe1aef7ea4dee2。

#### 2026-10-01T17:21:54.362161+00:00 AG071环境R1
- root实读workspace-sandbox.log五个已有HTTP service.rs:119监听EPERM，卡AG-071-R1.md SHA256f36826c8159b9cd97e248a46c5eb704de3e54eb3188389296addd328350adcf7授权原命令/原hash/原target申请执行环境复跑，初全量仍待结束，不能提前报计数。产品返修0，实际失败日志保留；若工具拒绝不得绕过。已发原agent max。

#### 2026-10-01T17:26:04.557989+00:00 AG069实际父双反例与基线
- root frozen append c5b72f7755ea3e6172856c6d3ae2d9c69aac904c77c7622c2c876c1a46634159、source f36a3c5b…在base212c784/独立target成功编译；ctrl-ag069-adversarial-base-212c784.log 0/2 exit101。两例确因Admin未起恢复轮仍Failed，不是编译/夹具设置错误，清理后WT clean。涉及真正200c旧DB复制，原DB不变；更高水位及Worker重开、多未知逐个修复在head尚未验证，不提前称通过。
- 实施方基线storage3/10 exit101，engine实际旧QA单项0/1 exit101，产品lifecycle f862740d…且diff0；新fixture SQL字段/CHECK设置静态校正未计红灯。root释放槽给069实施定向，df21GiB；071 full/070定向，≤3Cargo。

#### 2026-10-01T17:27:22.906005+00:00 AG070定向交付进展
- agent停止Cargo等待full：baseline2/0、新集成12/0、返回前DB提交边界3/0、五集成目标83/0、旧evidence4/0；去重算0/1、去末次DB复核0/3均编译后101、恢复字节62ba91a1cc06d24ce55808aaffa51450bc15c81479e3582f99b1e511ac68cb90。尚未主控最终验收/全量，不登记verified。df21GiB，待071冻结清其纯cache后分配full。

#### 2026-10-01T17:28:19.824155+00:00 完成基线缓存回收
- AG071 full同命令环境复跑1273/0，随后build/smoke；约20.64GiB。为保20GiB，root仅删已经结束的target-ctrl-base-ag069（双probe日志done/exit101、WT212c784 clean先核）；源码WT与冻结probe/实际红灯日志/原始旧DB完整保留。此可重建缓存提前回收沿17:08空间裁决，非验收完成；069若叠新父必须另fresh target，不跨树复用。df回21GiB。

#### 2026-10-01T17:31:09.064267+00:00 AG073缓存即时读门裁决待派
- SC-BROKER-CACHE两份原文完整归档SHAf4444d327510ad4b736cf544926e782bd79bd67d35153a0b8cf8c17f42519b39，最新修订撤回强制旧BudgetCallRef建议：root实读validate重建全source_closure摘要且call.actual_input_digest精确绑定，现有ref缺/空的合法恢复不能被无依据改写。只用已验证request全部run及upstream即可闭合缓存读门。
- AG-073.md正式范围卡SHA256cbae1fd0ced666fa0e2cfef9ec154a3962fd5971416c3290e12ba7ba9dc91304，基线212c784，E08/E03 §11L1077/L1110/L1132–34，V017/18子范围；只broker.rs及新test，尚未派发（当前069/070/071三实施）。等071冻结释放槽再派gpt-6.1-sol/max；不连带typed新版本/预算迁移。

#### 2026-10-01T17:32:09.832272+00:00 磁盘20GiB闸门实际触发与恢复
- df -k Available20240000KiB约19.3GiB（不能以df整数20GiB冒充达标）。070先前已明确全部Cargo停止、071父probe已结束；主控通知070后只回收target-ag-070约612MiB与target-ctrl-base-ag071约582MiB。WT/source/scratch/日志/probe/卡完全保留，无运行target被删。回收后21400000KiB约20.4GiB，已恢复底线；未启动新full，后续070从fresh重建。提前缓存回收属空间裁决，非任务验收或证据删改。
- 071build0/smokews0，CLI1日志只证明HTTP not ready，未打印子进程stderr，不能称这份CLI日志直接证明EPERM；依据已复現的同套localhost环境R1申请同命令复跑，仍待实际结果。

#### 2026-10-01T17:33:36.277789+00:00 SC-AG070-REVIEW只读派发
- 用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/max；卡638d1ce527de0a97ae441f0082c3fad4baffafdafb196db81f76128f8086e3f7。仅只读已暂停Cargo的ag070四文件现有hash，聚焦完整闭包/重建/limits/独立反例，不修改构建不下派；root仍独立完整验收。

#### 2026-10-01T17:37:32.412273+00:00 AG071 self cache回收及冻结head
- 全部required self构建/smoke退出已实读0；agent确认所有exec session完结、无后续构建，root只读提权ps仅命中查询自身。因系统空间再降至19880000KiB<20GiB，核产品91ab1d48…与test4f81e310…一致后删仅target-ag-071，所有源码/原失败/复跑日志保留。回收后22920000KiB≈21.86GiB。root full尚未启动，须fresh/限制并发并继续监测。
- 本地commit d8a2e4769879eced02be1937882dd08216ff24f4出现，+688/-6，两白名单文件；子代理最终report/push尚在整理，root尚未接管rebase。

#### 2026-10-01T17:44:59.793488+00:00 AG071交付及外发复核新事实
- 冻结d8a2e4769879eced02be1937882dd08216ff24f4、两白名单文件+688/-6、工作区clean；产品91ab1d48…/测试4f81e310…与root完整预审相同。报告及原始拒绝/所有顶层日志归qa/impl-ag071，report SHA256406e5e9b5ddec53e1e0054311fbfbd0038f252fed9be292b2da83e0f14fcab73。self全量1273/0实际基线200c，当前main1273/0另有068十项，root叠放后应1283/0。
- 子代理push自动审批拒绝原文已读，理由为缺可信payload/destination授权。root核本会话直接人类要求按哈希启动文§3.4与§10实施任务一PR并合并；实际payload只有已审privacy helper和纯合成新test，不含out/方案/archive/秘密。§7保密目的遵守，不将其文档枚举误解为禁止同文明确的产品任务。远端https://github.com/acosmi/RSIAgent.git，API public、owner acosmi、当前admin/push均true。以这些独立新事实向同一精确push请求复核，不换上传通道；未获准前不外发。
- 启动闸门哈希/工具均通过，main212c784不变。df24000000KiB≈22.89GiB；069收尾定向/变异，070暂停Cargo；root即将fresh full并使用CARGO_BUILD_JOBS=2降低资源峰值，所有八门与断言不变。

#### 2026-10-01T17:45:42.686452+00:00 AG071 root外发复核仍拒绝
- 同一精确push尚未创建进程，自动审批原由：该操作会将两份未公开源代码推送到公共 GitHub 仓库；用户授权了项目实施和 PR 流程，但未在可信用户内容中明确授权这份具体代码向该公共目的地外发。 原文qa/ag071-root-push-approval-rejection.txt。不再重试/换通道。先完成本地可审阅独立验收，再向用户提出具体payload/public目的地授权；外发挂起不停止069/070/073和本地验收。AG071无PR/未合并，不更新为已验或已合并。

#### 2026-10-01T17:47:10.236696+00:00 AG069 R1旧断言范围裁决 / AG073派发
- AG069-R1.md SHA256f5a48d3cb025a9bcf36eb72da368ce2f8833d4f3eb0fd3612e20d41996b2fee2。root实读未知late依赖旧测试与真源，唯一末尾Admin page1即时Failed不符合已裁定有界全frontier重扫；授权该单测试末块改Worker不启动+Admin完整轮无Complete且仍Failed、未知body/identity/水位保持，其余逐字不变，原71/1日志保留。
- AG073卡SHA256cbae1fd0ced666fa0e2cfef9ec154a3962fd5971416c3290e12ba7ba9dc91304，基线212c78488b403397eaebaa65a6d280b9c6be3899；WT ag073/branchwrokbot/ag-073-cached-model-revoke-gate，/root/impl_ag054（沿用gpt-6.1-sol/max），E08/E03 V017/V018 Completed缓存撤销读门，两白名单文件与069/070互不重叠；实施3。当前仅授权读/写卡内新tests及静态准备，不启动Cargo，待root分配；外发审批现有阻塞下先只交本地冻结提交，不push/建PR。
- AG071已本地叠至53dd17677a38abab26d31f1a57778b7fca9694e6，实际父212c784，唯一patch cmp0；因从未push无remote lease改写，外发仍挂起。fresh target-ctrl-ag071独立八门运行session22740，jobs2、开始23.621GiB，预计1283/0，未先宣称通过。

#### 2026-10-01T17:51:20.474631+00:00 AG070侦察归档与root独立反例准备
- qa/probe-ag070/source-import-v41-212c784.rs SHA256c6d4f537a0323a67e7cc40753f0d94682fb360db20532a7c9d3539de30373dd2。
- qa/probe-ag070/append.rs SHA2562abd0ca8c480973dd40ec93d282a46ca5cb8c9ea7fb687de0669492dcd92324f。
- qa/probe-ag070/head-new-api.rs SHA256c9267fbcda7a708dab61712f9a89877170ef6eec5cc0e27c2ae4edcd39dbdabe。
- cards/SC-AG070-REVIEW-r4-report.md SHA25685741aef05e382cfe2e15c986d69fc5bc5908f4a0fb3e950b159c0ab7426c121。
- 三产品hash静态复核未发现具体漏洞；test从1f50…变d92b…，不能认同一冻结快照，待实施正式冻结重审。root自写两共同API边界：完整refs交换重算input仍拒、未选second blob改为内容相同的受控symlink仍拒primary片段且恢复后成功；两额外new API边界：重绑顺序拒绝、仅转义metadata超128KiB返回零excerpt/partial/cursor0。全未运行，不冒称通过；旧API双方预计通过是边界非产品红灯，新API只head运行不把缺符号编译错误当红灯。

#### 2026-10-01T17:54:07.434757+00:00 CTRL-AG071-R1 本地独立验收通过、外发挂起
- 本地验收固定head `53dd17677a38abab26d31f1a57778b7fca9694e6`，实际父 `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `596687aa3d8380ee70ba4a121a90bcb42edf05bd`。原d8a2e4769879eced02be1937882dd08216ff24f4从200c43bc19368ff52b064e67eb1b14e35f096d39叠放，单提交before/after-rebase.patch逐字节cmp0；无冲突、WT clean。尚无远端branch/PR，因此未执行force-with-lease、ready或merge。
- 两白名单文件+688/-6完整预审，交付与冻结SHA一致。产品仅private UTF8 endpoint helper和3调用(+12/-6)；new privacy_utf8_v42 676行10测试。ASCII窗口12/24/16、marker/顺序/等级/错误/时间policy不变。
- 全新target-ctrl-ag071独立八门全0，ctrl-ag071-53dd176.log*；逐行独立求和97结果行=92测试二进制+5doc组，1283/0=实际父1273+10，Python48。CARGO_BUILD_JOBS=2仅限制构建并发，所有要求命令/断言保持。root localhost完整全量/双smoke实际通过。
- 两root自写反例：frozen source e4c6a606f217a36c29f8f800c8e5e7024ddfac2b22e922c7224187d62d07c4f7，append b136b3ba853541286710b15713b23cb1f77785351820bed1c02d38dea95a3b05；head ctrl-ag071-adversarial-head-53dd176.log 2/0 exit0；实际父 ctrl-ag071-adversarial-base-212c784.log 0/2 exit101，双方成功编译且WT clean。P1真实export metadata name/dependency.version_req/nonprimary source-id共9输入JSON往返，P2foreign approval/evaluation notes共6输入manifest往返；旧树15实际UTF8 panic，head均精确privacy Error，入参字节不变、Clean正例仍通过。最初root ForeignMetadata::default编译设置错误保留invalid-default-setup，不算产品红灯。
- 实施报告qa/impl-ag071/implementation-report.md SHA256406e5e9b5ddec53e1e0054311fbfbd0038f252fed9be292b2da83e0f14fcab73；最终baseline2/8、504边界184panic、固定10/0、38ASCII findings逐字段快照、两变异101与cmp0已核。首新test误把C:/Users/双命中当单命中只修新test且重跑原product baseline；原错误保留。初workspace1268/5明确HTTP EPERM，R1同命令1273/0；CLI初1仅readiness超时不伪称stderr EPERM，提权同命令0。
- 本地verified子范围仅§11.2L1104/E16.2L1427,L1429/V067已知隐私模式的UTF8安全阻断。不声明更广扫描覆盖、绝对无泄漏、授权出站、完整导入学习链/V076、V075新验证、真实模型/效果/发行。privacy报告本身可能含原snippet，政策未改。回滚参考父212c78488b403397eaebaa65a6d280b9c6be3899，不还原已脱敏内容、费用、审计或撤销事实。
- 产品返修0、R1环境裁决1，卡e20c8b42c67a88fbc9072491947da2b82396b9e3a445b288d89cdde5512c56a9/R1 f36826c8159b9cd97e248a46c5eb704de3e54eb3188389296addd328350adcf7。child与root两次原精确push均在创建进程前被自动审批拒绝，未外发、未绕通道；root新事实复核仍要求具体payload公共目的地授权，待用户明确。无PR/未合并/不可填写merged_sha或纳入已合并汇总；保留完整本地结果可复审。

#### 2026-10-01T17:56:49.910658+00:00 SC-IMPORT-PROJECTION派发
- 卡SC-IMPORT-PROJECTION-r4.md SHA25684d1ce89aad7f73b774a98ec641eb80e044802d4c3bb552004ce814ec7b16054，用户授权辅助chat gpt-6.1-sol/max，只读不构建；继续把导入链后续安全投影细化为可实施范围，不以扫描Clean/固定计数冒称完整学习。069/070/073三实施最多3Cargo；root071已本地验收，无root运行编译。

#### 2026-10-01T17:58:02.727039+00:00 AG069完整预审及资源协调
- root完整读lifecycle +130/-4、新storage1187行15tests、新engine485行2tests、两个旧test最小授权diff；未发现产品范围外变化。旧AG068其它九test/helper逐字约束保留。R1追加cleanup_fixpoint单末块+35/-2核Worker不启动、Adminpage1 Running再完整轮仍Failed/无Complete/原body和identity水位不变。clippy初len_zero101仅新test同义!is_empty修正，storage新SHA ce489ac100881f85cce249f034374ebc07261dffea59f87e5cb92032146f9386；原首错保留，full需实际测新字节。
- 069 self full与070 self full、073第三定向槽最多3；root不启动第四路。071八门/probe所有session已完成且WTclean，提前移除仅已本地验收target-ctrl-ag071以给在途构建留量；未合并WT/branch及所有QA保留，若父变化必须fresh重验。
- 冻结069提交候选的新旧程序生成测试源码qa/probe-ag069/source-upgrade-generator-1741390f.rs SHA1741390fe71465747b3ba7471a2fb8205b3fe8bbc1da366af72871474cf41c16，root准备另在真实200c旧树运行其可选生成入口；尚未运行，不冒称新生成器已验证。已有原始真实oldDB4fdb…继续原样保留。

#### 2026-10-01T18:01:08.521718+00:00 AG069 R2第二处旧断言授权
- AG-069-R2.md SHA256c97c91f5bb98b7fc680f9bd0c73e82349cd960ec4a6e070dc59f365f710d4c41。curriculum_cleanup_v42两个未知测试/三个helper调用末尾即时Failed与完整frontier分页冲突，root实读失败和helper，按R1同真源仅授权assert_blocked末块Worker不启动+Admin有界整轮仍Failed/每页无Complete/精确错误与身份水位保持。原全量仍运行，禁止中途改源，原失败保留；初次断言与其它全文逐字不变。五HTTP EPERM另环境裁决，不能混淆。

#### 2026-10-01T18:01:46.764439+00:00 AG069 R3环境全量裁决
- 原workspace结束98结果行1283/7由root独立求和，2旧断言归R2、5HTTP listener EPERM环境；AG-069-R3.md SHA25632e25b40c92411cc5c059b27a27bdf6d5d97b709fe28d26ca071da6baf72ae40仅允许R2定向通过/最终hash冻结后同full命令target环境提权申请，不过滤/改产品；原失败保留。产品返修0、测试授权2、环境1，full实际仍待验。

#### 2026-10-01T18:03:48.782745+00:00 AG070 R2环境 / AG073 root探针冻结
- AG-070-R2.md SHA256b3dbfbc55c9f73d5062a7bb16f3fa675a95560770d502dcfcf84da57cbde975f，root独立原self full96行1273/5=1278总；只有五HTTP EPERM，原200c基线1263+新15，head当前main未来应1288总。四source不变同命令环境复跑获授权申请，不过滤不外发。
- root probe-ag073/source-broker-212c784.rs SHA2569cb8c4a2de0f858ad5dcdbde2f90f0c38695bd8dae78276cdc79fa51277fecdc；append.rs SHA2563b8003c107516590e63e2aab513026853afc81a30d4f2088215b2cf0b926b24b。P1缓存之后才加到已撤销artifact的迟到依赖及环、重开disabledbroker仍须拒；P2已经stop root/group且旧lease过期时live缓存照常读，之后来源撤销必须拒且历史回执/实际费用不回退。尚未编译运行，不能算通过。

#### 2026-10-01T18:13:00.953239+00:00 续跑闸门与AG069真实旧生成器
- 提示词b0000207…/真源70ec06e4…/archive45f3ba06…匹配，auth/fetch0、cargo1.98.1、Python3.12.14、磁盘约27.8GiB。main212c784保持、goal active，用户未收尾。
- root在200c43bc19368ff52b064e67eb1b14e35f096d39真实运行冻结生成器1741390f…成功1/0 exit0；ctrl-ag069-old-generator-200c43b.log/exit/status，旧WT清洁。新增独立backup old-failed-ag069-agent-generator/rsia.sqlite3 SHA58902f602dba9e1a51dc1f5468092142bc919a5a03b1854827d51046aee4668f，来源真实Host写入+明确typed夹具边+cleanup+consistentbackup，未SQL捏造state；Failed processed6/pending0/source run secondary/watermark2，三native原文仍在。原4fdb653a…旧DB不变；head升级验证待交付，不提前称修复通过。已归还073第三Cargo槽继续变异/定向。
- SC-IMPORT-PROJECTION辅助chat完整原文归cards/SC-IMPORT-PROJECTION-r4-report.md SHA2560562f8091a40778380d227c79dabb43eace55e4dada5222769fa4c4ca49db284，root完整实读。实际摘录没有通用脱敏；方法有限句式/全字段封闭投影与真实计量区分工程参数和既有Admin授权，后续须独立真源裁决。
- AG070 self环境重跑1278/0、build/双smoke0待root独立核对；069 R2定向85/72/27均0后R3全量在途；073变异在途，root静态读到临时take(1)不当作最终产品。外发仍等先前具体许可问题，未经回复不重试。

#### 2026-10-01T18:15:34.987110+00:00 只读typed边侦察与070父探针
- 用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc继续SC-TYPED-EDGES-r4，gpt-6.1-sol/max，卡SHA3973a65eed70529424d1e0acc11801e41055e80b561aabd9f7f225cc552e69c3；基线main212c784，E08L1295/L1299与§6.3/§11，限定现有E16三个schema的有界只读检查器可行性，不替代完整导入链、不写产品。
- 070 self所有构建完结，root创建base-212c784-ag070与全新target-ctrl-base-ag070，冻结共同旧API双probe开始真实父运行session53653；未知结果，若两边通过如实记边界。当前Cargo069/073/root三路，无第四路。

#### 2026-10-01T18:17:23.935708+00:00 070父边界与073预审/全量许可
- root070共同旧API两probe在实际父212c正常编译2/0 exit0，日志ctrl-ag070-adversarial-base-212c784.log、WTclean；这是边界核验，不能声称基线产品缺陷。head共同两项与新API额外两项仍待运行。
- root完整预读073产品34/-3与新test1015行20项，恢复后SHA6328e3c49081c36e3d3ea4817686dbdab937a624b911829a4ab64c4260de1d65/93cfb1e386a3a5417617c29bf54ec4ba6572acaf439804b32a72f83e11d267da一致。其移门/仅primary两变异正常编译9/11、恢复cmp0/定向52/0，允许jobs2完整clippy/full/build/smoke，同树不跨target；未独立验收、不先verified。
- root独立求和070self环境full96结果行1278/0（91bin+5doc），四文件hash与此前全文预审一致，唯一提交b895934ce1d18c64766d26a958661bdaf717b573/+1971/-2，等agent补finalreport/交接后本地叠放，不外发。

#### 2026-10-01T18:20:23.108903+00:00 AG070本地交接与叠放
- 最终report16062df0091f9a22681e81342d302597f8f667d30c719433a7ff13bbca99a7bb及顶层原日志完整归qa/impl-ag070；唯一提交b895934ce1d18c64766d26a958661bdaf717b573自200c重基至212c784→ac90c02bca65a4630ebc51f5518dc406c7038599/tree36781065c859ccdd95f1afff331db68697e6c09b，前后单patch cmp0，无冲突、WTclean。仅本地未push/无PR，不需remote lease。
- agent明确全部Cargo/CLI/session结束；其旧源码树cache target-ag-070已提前回收，所有源码/报告/QA保留，不可跨树复用。root全新target-ctrl-ag070独立八门即将启动，预计当前父1273+新15=1288/0；在full期间不放probe，尚未验收。

#### 2026-10-01T18:23:20.895264+00:00 AG074派发及073环境R1
- AG074 E09/E10 V020/22/23/25/81/86正常Deepen前缀限定；卡SHAee6487be89271262896ef9f63869c23a25b46d86512260462a47838cad72a6f6，基线212c784，WT ag074/branch wrokbot/ag-074-deepen-expanded-parent；沿原/root/impl_ag057 gpt-6.1-sol/max。主控裁决在线已有任意子节点耗父机会按§7.1.1共享核心/无虚构分支目的同步core与replay，非真源逐字合同；一般终态/评分另项不动。当前只静态/新test准备，不准Cargo，实施069收尾/073/074共3；070root独立full并行。
- root独立核073原workspace1288/5，全部5HTTP EPERM；R1 SHA38fa9f0f29ba66c11122c13f9f5a88fadbff0134df3675d24ad8890c81f2c66a允许同原full环境复跑，不改代码/过滤。产品返修0。
- root073探针静态复核发现原late-origin artifact夹具不符合begin_revoke严格envelope，未执行前已修为精确E16 import_source；旧append保存before-artifact-fixture-review，新append SHA79c652447b6c2c87d2120cf3e04cddf438d2fe60fb0f81001e087e040fa05d13，尚未编译/未称红绿。

#### 2026-10-01T18:25:59.643804+00:00 AG073主控实际父双反例
- frozen source9cb8c4a2…及修正版append79c652447b6c2c87d2120cf3e04cddf438d2fe60fb0f81001e087e040fa05d13在base212c/独立target正常编译后0/2 exit101；ctrl-ag073-adversarial-base-212c784.log且WTclean。P1缓存后撤销尚无关artifact的正例先成功，迟到环边接到它再重开Store应拒却未拒；P2根预算/group停止及租约过期的合法旧事实缓存仍应能读，实际源撤销后应拒却未拒。与实施测试的来源/停组/late边角度不同。head待验，不先称通过。

#### 2026-10-01T18:26:40.834511+00:00 AG069交接及缓存空间恢复
- 唯一head52de2d76b47793e9212ec81568d5d959061934d3、父212c784、tree8c8fcd88c522b784191901cd55682ead083ec7fd、6文件+1923/-27已完整预读核最终hash/授权旧测试末块。原agent明确无活动Cargo；report与顶层原始证据归qa/impl-ag069，原scratch保留，root独立self计数98行1290/0。无PR/从未push，publication挂起。
- df最低本次21840000KiB≈20.82GiB；提前仅回收已停止069 self cache与已完成的070父/073父/069旧生成器cache四目录。各WT/分支/原DB/报告/完整QA保持；全部为可重建缓存，未称这些任务已合并，不跨树复用。root069将全新target独立full，070full仍在运行、073R1环境full在运行，最多三Cargo；074继续静态等待。

#### 2026-10-01T18:31:53.822789+00:00 CTRL-AG070-R1 — 本地独立验收通过，外发暂停

固定head ac90c02bca65a4630ebc51f5518dc406c7038599，实际父212c78488b403397eaebaa65a6d280b9c6be3899，tree36781065c859ccdd95f1afff331db68697e6c09b。无PR、未push/ready/merge，不计已合并任务。

原b895934ce1d18c64766d26a958661bdaf717b573从200c43bc19368ff52b064e67eb1b14e35f096d39本地重基至当前main212c784；唯一提交前后patch逐字节cmp0，无冲突。未有远端分支，无force-with-lease动作。四白名单文件+1971/-2完整逐行预审，最终hash与预审一致；三个原有产品块及原测试逐字保持，仅新增本地消费者/合同/私有三测试及1018行集成12测试。交付/探针后WT clean。

全新target-ctrl-ag070，ctrl-ag070-ac90c02.log*八项exit0：fmt/clippy/workspace/build/双smoke/support checker/unittest；root独立求和97结果行=92测试二进制+5doc组，1288/0=实际父1273+15（12集成+3返回边界），Python48。jobs2只限制资源，不过滤测试。

root冻结source-import-v41-212c784.rs SHA c6d4f537a0323a67e7cc40753f0d94682fb360db20532a7c9d3539de30373dd2；共同append.rs SHA2abd0ca8c480973dd40ec93d282a46ca5cb8c9ea7fb687de0669492dcd92324f。
- P1两个来源完整source_refs换序且重算input_digest，旧公共load_live_result重复请求仍精确拒绝source mismatch，artifact/edges/auditcount不变。
- P2未选第二来源的受控blob换成内容完全相同的symlink，读取主来源fragment仍拒绝且DB/canary不变；unlink还原原文件后正向再读成功。
- 实际父ctrl-ag070-adversarial-base-212c784.log两项2/0 exit0；head共同两项也2/0。两树同一冻结源码/append均正常编译、事后clean。属于边界核验，不冒称旧产品缺陷红灯。
- 新API额外Q1重绑全部请求摘要后新consumer仍拒换序闭包；Q2单metadata的30000个NUL经JSON转义已超过128KiB，原raw片段仍可读，新材料返回零摘录/实际序列化数组2字节/partial与cursor0/全部来源保留，DB不变。
- head-new-api.rs SHA c9267fbcda7a708dab61712f9a89877170ef6eec5cc0e27c2ae4edcd39dbdabe；共同与额外拼接append-with-head-extras.rs SHA cb7b2f1486aeee844d1f75855cd5ba406cb11f0b880ceb2390a677037c022f49。ctrl-ag070-adversarial-head-ac90c02.log共4/0 exit0。旧树没有新接口，两额外项只验head，不用缺符号编译错假造基线。

实施报告qa/impl-ag070/REPORT.md SHA16062df0091f9a22681e81342d302597f8f667d30c719433a7ff13bbca99a7bb已完整读。其旧基线公共API真实2/0；新增首错9/2为SQLite id CHECK夹具，纠正后12/0。删重算0/1、删返回前DB复核0/3均正常编译后101，精确恢复；最终hardlink断言已真跑12/0及两次full。初full1273/5为五个旧HTTP EPERM，R2同命令完整1278/0（该树父200c1263+15）；初CLI只readiness1未打印stderr，不单独称EPERM，同命令环境0。所有原失败/工具设置失败保留，非产品返修。

verified仅E16.1独立导入材料读取段：§6.3L629/L644/L646/L648/L650、§6.4L658/L684、§11L1077/L1110/L1132–1134、E16.1L1419–1423；V005/V006/V017/V051–56/V076限定已实现三固定reader register→execute→真实typed本地consumer与来源活性。原owner Admin、Development、全result/selection/来源对象fingerprint、注册授权与reader/派生内容重核；未选/聚合/不可读源都保留闭包；实际blob一次读取及共享上游撤销门；返回前重载全DB对象。固定ImportedHistory/UnverifiedImport/Development、Unreviewed/NoModelDispatch，不改变旧G1。

边界：64MiB读取预算对每Ready源保守多计1探测字节，不能完整重核则明确拒；摘录数组≤128KiB/≤32，coverage与原import分开。内容仍可含敏感值/命令/答案，只限本地域，未调用scan当安全证明。R1明确固定Vec是字节快照，不能证明复制后无DB变化的外部同名文件永远不变；未来consumer必须重新读取并核授权/撤销。cursor仅信息、不支持offset重入，core validate只是形状/摘要校验不是存储权威。返回不是长期授权；事务快照之后发生的撤销不作跨时线性化保证。

不可声明安全出站、持久Pattern/RoutingDecision、E16生成/typed Broker预算/真实Skill消费runner/Candidate/formal/new-run、完整V076或完整默认来源支持、真实模型/收益/发行。完整学习链仍必须继续实施，不因本段通过标完成。回滚参考实际父212c784，仅移除新增API/类型，不逆转已发生费用/撤销/历史或恢复明文。

卡2dbd6904f5271a4a1d5c8ea16c27398afac8f7e56d7241a8ebff07b0e35e9127；R1字节快照93acd38dbd2cbebc376735a837346dae909470b0ce89932aeb3fd47067ef484f；R2环境b3dbfbc55c9f73d5062a7bb16f3fa675a95560770d502dcfcf84da57cbde975f。产品返修0，合同纠正1、环境裁决1，无冲突/admin未用。AG070自身未遭外发工具拒绝，因为遵照root统一暂停没有尝试push；AG071精确公共外发双重拒绝待用户回复仍有效。后续若main前进，必须重基并全新独立验收再合并。

#### 2026-10-01T18:32:37.933225+00:00 AG070本地验收缓存与AG069额外旧库父对照
- AG070全量与全部4probe已完成、WTclean、本地验收清单958f8ad045da93963bdb1f48b0e617a3ff67b86ab7c3044498df7e6ee16e22ba；仅回收已停止target-ctrl-ag070保持磁盘，源码/父WT/branch/QA保留，未合并。以后main变更须fresh再验。
- AG069新真实200c生成DB58902f60…的额外父212c升级对照开始，用全新target-ctrl-base-ag069-upgrade，原冻结完整generator/test1741390f…与env只读取旧产物再copy；不能和原4fdb65…root双probe混为一份DB。当前Cargoroot069full/074baseline/root此父对照最多三路，073仅smoke/report。

##### 2026-10-01T18:40:01.834153+00:00 资源协调
- 074报告Available19.61GiB及时停新Cargo；root069补充旧DBhead session66488已最终0。073最终报告已核4d579564…并归档全部顶层证据，agent确认全部构建停止，回收纯target-ag-073及已结束target-ctrl-base-ag069-upgrade；源码、分支、WT、原DB与全部QA保留。后续root073使用全新独立target。

#### 2026-10-01T18:43:27.968590+00:00 AG073主控全量与记录脚本纠正
- frozena9893b0、父212c、clean、源码hash与逐行预审一致；最终report4d579564…已全读归档，fresh全量session43609已启动，localhost环境获准、不外发，结果待实际日志。
- 同工具前置记录脚本因提权环境python3落到旧解释器报Non-UTF-8 SyntaxError，未写state/ledger；后续bash full独立启动并在跑，不能重复启动。现用明确out/pybin/python3补记真实事件，记录脚本初错不算产品失败。
- SC-TYPED-EDGES完整原文已保存并阅读，后续按真源裁决；不重复巨大tool输出，不依据被截断日志验收。

#### 2026-10-01T18:45:45.248968+00:00 CTRL-AG069-R1 本地独立验收
# CTRL-AG069-R1：旧 Failed 作业显式 Admin 续清，本地独立验收通过

固定 head `52de2d76b47793e9212ec81568d5d959061934d3`，实际父/当前 main `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `8c8fcd88c522b784191901cd55682ead083ec7fd`。唯一提交、无需重基，6 个白名单文件 +1923/-27 已逐文件完整阅读并核对最终 hash，交付及探针后 clean。未尝试 push、无 PR、无 merge；不可登记 merged_sha，不计已合并产品数。

产品在现有 cleanup_step 内仅允许 Admin 对 Failed 显式启动完整恢复轮；Worker 不新开轮但可续 Running。保留原身份/水位/创建时间及所有费用/历史，原 processed/error 入 retry 审计后重算本轮进度，重置完整 frontier 的 expanded 和两个扫描 cursor。只有真正 Complete 同事务清 last_error；仍未知则再次 Failed。旧 export 副作用在 absent/redacted 早退前按同 namespace 和 typed node 的精确旧错误事实重新检查，跨 job 共享记录也有效；不只查最后错误、不读用户任意路径、不一次载入历史，不改未知 wire 分类。

主控全新 `target-ctrl-ag069` 独立复跑，`qa/ctrl-ag069-52de2d7.log*` 八项退出码全部 0：fmt、clippy、workspace、build rsia、两组 smoke、support checker、Python unittest。逐行求和 98 结果组 = 93 测试二进制 + 5 doc 组，**1290 通过 / 0 失败 = 父 1273 + 新 17**（storage 15、engine 2）；Python 48。默认全量里的可选旧 DB 生成入口不等于真实旧程序升级证明，独立证据如下。

#### 主控两项冻结反例

共同源码 `probe-ag069/source-native-cleanup-f36a3c5b.rs` SHA256 `f36a3c5b808c68d7e74c628475bd796a4b412ebe83443227db96c37b2f0b6329`，append SHA256 `c5b72f7755ea3e6172856c6d3ae2d9c69aac904c77c7622c2c876c1a46634159`。两树正常编译、同一源码/append：head `ctrl-ag069-adversarial-head-52de2d7.log` **2/0 exit 0**；实际父 `ctrl-ag069-adversarial-base-212c784.log` **0/2 exit 101**。父均因 Admin 仍 Failed 不能启动恢复而失败，非编译错误。运行后两树 clean。

- P1：真实 200c 旧程序生成的 Failed DB，在另一作业已抬高 namespace 水位后，Admin 一次启动旧作业，随后 Worker 每页重开 Store 续清到 Complete。原 job/source/watermark/digest/history 保留，另一个作业保持 Pending，已脱敏对象不重哈原始摘要。
- P2：旧恢复轮内，在已 expanded 父之后插入两个实际 Host writer 创建后故意损坏的迟到 authority 夹具。新未知使作业再 Failed；只修最后错误仍不能 Complete，分别全部修复再 Admin 重试才完成；旧失败和 retry 事件不消失。这些人工 typed 边/损坏是清理反例，不能冒称生产学习链。

两项使用原始 `qa/old-failed-ag069/rsia.sqlite3` 的临时副本；原件 SHA256 始终 `4fdb653a0f78fbefa3ef813522564a6f09e929736e95df2fe069b7ef04228b4a`。该库由主控实际旧树 `200c43bc19368ff52b064e67eb1b14e35f096d39` 真实 Host 写入、begin_revoke/cleanup、关闭/一致性备份生成，无 SQL 伪造 Failed；初始 processed8/pending0，三个 native 原文尚未清。

#### 补充真实旧程序产物升级

主控另在实际 200c 旧树运行实施方冻结源码 `probe-ag069/source-upgrade-generator-1741390f.rs`（SHA256 `1741390fe71465747b3ba7471a2fb8205b3fe8bbc1da366af72871474cf41c16`），`ctrl-ag069-old-generator-200c43b.log` **1/0 exit 0**，得到独立备份 `old-failed-ag069-agent-generator/rsia.sqlite3` SHA256 `58902f602dba9e1a51dc1f5468092142bc919a5a03b1854827d51046aee4668f`。

同一备份副本/同冻结测试：实际父 `ctrl-ag069-new-old-db-parent-212c784.log` 正常编译后 **0/1 exit 101**（Failed != Running），head `ctrl-ag069-new-old-db-head-52de2d7.log` **1/0 exit 0**。两份日志明确 actual_old_program=true；head 从 Failed/processed6/pending0/三个未清 native 到 Complete/processed6/pending0/last_error None，job `revoke-53d5d5a9c604a2e5b56b6c1dd9a8ff32`、source run secondary、水位 seq2/digest f2a910ba…保持。原 DB 两份 hash 独立复核原样不变。不混淆两个真实旧库，也不把默认未设置 env 的新库测试当升级证据。

#### 实施证据与裁决

`qa/impl-ag069/final-report.md` SHA256 `836a867fd5104a3315a833a0a30ddb029f2d440a5a63c295d202e4cea6f13f22` 已完整读，所有顶层原始证据归档。storage 基线正常编译 3/10，真实旧库基线 0/1；修正新增 fixture 的水位 hash 设置后定向 27/0。三个正常编译产品变异分别 13/14、22/5、26/1，exit101，恢复 cmp0。首次 clippy 仅新测试 len_zero，等价 !is_empty 修复后全部重新验证。

R1 授权 cleanup_fixpoint 唯一测试末块，R2 授权 curriculum_cleanup_v42 的 assert_blocked 末块：原 Admin 再调用必须立即 Failed 的断言与完整有界重扫不相容。保留 Worker Failed、不出现 Complete、整轮终态精确 Failed/错误/身份/水位和原未知正文；未授权区块逐字不变。卡原授权 AG068 单个旧 Failed 测试改为合法恢复，其他九项与 helpers 不动。产品返修 0，旧测试范围裁决 2。

原 workspace **1283/7** 的两项旧语义断言与五项 HTTP EPERM 分开记录；R1/R2 定向通过后，R3 同完整命令环境复跑 **1290/0**，build/两 smoke 0。默认 CLI readiness exit1 的原日志单独不能证明 EPERM，不伪称其 stderr；环境重跑实际 0。原失败、脚本设置错误与恢复证据保留。

卡 SHA256 `b988de7f2245c7b495f5fa013eb930c0b48505d14de45725feb1dddf67bd5c30`；R1 `f5a48d3cb025a9bcf36eb72da368ce2f8833d4f3eb0fd3612e20d41996b2fee2`；R2 `c97c91f5bb98b7fc680f9bd0c73e82349cd960ec4a6e070dc59f365f710d4c41`；R3 环境 `32e25b40c92411cc5c059b27a27bdf6d5d97b709fe28d26ca071da6baf72ae40`。

#### 结论与边界

本地 verified 仅 E08/E16.5，§11 L1069–1081 特别 L1077、§11.3 L1110–1112、§11.5 L1132–1134、E08 L1295/L1299，V017/V018/V038 的本地持久清理恢复子范围：在现有可信控制事实下，旧 Failed 通过显式 Admin 重扫完整 frontier，在已支持分类和受控 export 副作用验证完成后真正 Complete，分页重启可续且费用/历史/撤销不回退。

EXISTS 不代表常量或有界 SQL 扫描成本；文件系统与 SQLite 非原子；ensure_logical_block 的既有 tombstone 正文绑定不在本卡增强；支持分类之外仍 Failed。不声明未知 schema 自动修复、缺 anchor 恢复可信、所有 Pending 读门、完整备份链、物理擦除、第三方销毁、完整 V076 导入学习链、真实模型/收益/发行。

回滚参考实际父 212c784，仅撤销实现，不恢复明文或回退任何水位、账单、审计、已发生动作。无冲突，admin 未使用。AG069 从未请求外发，无本任务自动审批拒绝；遵照 AG071 公共外发拒绝后的统一暂停保留本地，待授权。若 main 前进，必须重基并全新复验再合并。


- AG069全部root full/probe/oldDB session均结束、本地清单冻结后仅回收target-ctrl-ag069；WT/分支/旧树/全部日志/原DB保留，尚未合并。

#### 2026-10-01T18:48:55.176299+00:00 AG075派发
- E08L1295/1299、§6.3L644/648/650、§11L1077/1081/1110/1112/1132–1134；V017/18/56/75限定已引入E16三schema声明直接依赖诊断，非完整V076。卡AG-075.md SHAf3a6621e090f0a04810b9a2f63f61e2179bb325e02852237e24611a237befe46；只读侦察原报告532ca655ddcb15151a19bcc5f00db99819cd80f96561a4dfa93e851da9033bad已完整保存/root阅读并实核writer/存储接口。
- 基线212c78488b403397eaebaa65a6d280b9c6be3899，WT ag075/branch wrokbot/ag-075-import-dependency-inspection，target-ag075与scratch/ag-075-impl；/root/impl_ag065复用gpt-6.1-sol/max。6白名单，storage/engine新模块+各lib一行+各新test，与074/冻结069/070/071/073不重叠。只本地commit/report，禁止外发。先静态再申请第三Cargo槽，全量另协调。
- 明确主控工程限制对象202/边10000/单页256为单次诊断读取预算，非新增持久容量合同；Prepared摘要过渡unknown，redacted只历史声明、物理blob not_checked，多余边不自动删、不修复、不当长效授权。全部source含计数/失败，无界dependents仅小基线fixture，产品必须SQL分页。
- AG074最终产品diff0的core基线0/2、engine1/3正常编译，真实旧引擎生成三类report6文件1/0；继续授权守卫与定向双变异，未准full。

#### 2026-10-01T18:49:28.592059+00:00 AG073主控全量通过
- 全新target完整八门全部exit0，ctrl-ag073-a9893b0.log*；独立97结果行1293/0=父1273+新20（92bin+5doc），Python48。session43609最终0且WTclean。现在同冻结head运行独立两反例；尚未以全量代替反例验收。

#### 2026-10-01T18:52:24.133061+00:00 CTRL-AG073-R1 本地独立验收
# CTRL-AG073-R1：Completed 模型缓存即时来源撤销读门，本地独立验收通过

固定 head `a9893b0bcbb28d9c50bc37d5ad851bcf840ba137`，实际父/当前 main `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `3ff2c03a14e4f26709e299d871019e851cb3a488`。唯一提交，无叠放/冲突。两个白名单文件 +1049/-3 已完整读审，最终 broker SHA256 `6328e3c49081c36e3d3ea4817686dbdab937a624b911829a4ab64c4260de1d65`、1015行新20测试 `93cfb1e386a3a5417617c29bf54ec4ba6572acaf439804b32a72f83e11d267da` 与完整预审一致。交付/探针后 clean。无 PR、未 push/ready/merge，不计已合并任务。

product仅 existing_response 在非redacted缓存先解析并validate_against，对 Completed 通过已绑定请求的全部 source_closure 映射 run/id，使用同一当前namespace事务内现有直接与upstream判定。门位于恢复 close_budget_call_execution 之前。撤销命中固定 `model_response_source_unavailable`；超10000沿用共享精确Conflict；数据库错误不吞。旧BudgetCallRef缺失/空/损坏不能缩减读取闭包，不额外新增来源存在性/可信性校验。

全新 `target-ctrl-ag073`，`ctrl-ag073-a9893b0.log*` 八门实际 exit0：fmt/clippy/workspace/build/双smoke/support checker/unittest。主控独立求和 **97结果组 = 92测试二进制 + 5doc，1293/0 = 实际父1273 + 新20**；Python48。未复用实施target，jobs2仅限制构建资源，不过滤测试。

#### 两项主控独立反例

冻结旧API源 `probe-ag073/source-broker-212c784.rs` SHA256 `9cb8c4a2de0f858ad5dcdbde2f90f0c38695bd8dae78276cdc79fa51277fecdc`；同一 append SHA256 `79c652447b6c2c87d2120cf3e04cddf438d2fe60fb0f81001e087e040fa05d13`。

- P1：真实结算Completed后，先撤销尚无关的合法E16 artifact，缓存仍成功（无关水位正例）；之后才注入来源→中间节点→已撤销artifact及环的迟到typed边，重开Store、Disabled transport、新clock重读必须拒绝，不再派发、不修改已发生费用/回执/审计。故意迟到边是门禁夹具，不冒称正常生产生成链。
- P2：真实Completed后根预算和stopgroup均停止、旧lease过期，未撤销来源的缓存仍返回旧内容（停止/过期不自动抹去历史）；实际来源begin_revoke后不做cleanup，两次重放必须拒正文。公开verify_receipt仍验证已发生事实，原费用/dispatch/调用数1保持。

head `ctrl-ag073-adversarial-head-a9893b0.log` **2/0 exit0**；实际父 `ctrl-ag073-adversarial-base-212c784.log` **0/2 exit101**，双方成功编译、同一source/append，父均在预期拒读断言失败，非设置/编译错，事后clean。初版root静态复核发现普通artifact不能作合法撤销source，在任何运行之前改为精确E16 envelope；原append保存 `append-before-artifact-fixture-review.rs`，不把未运行版本算失败。

#### 实施证据与裁决

最终完整报告 `qa/impl-ag073/implementation-report.md` SHA256 `4d579564b18c08dc60d6ad9a7fd8b4f3c59534a7f8feec1b5863a6af491d65a7` 全读；65项证据清单 `evidence-manifest.json` SHA256 `07c3a6bec86bf9ef3379599e8ba4666a560f16371a101f03691ccea7794f0fe4` 逐条核对应归档大小/hash。

未改产品基线20项 **9/11 exit101**，固定20/0；删除门及只查primary两个变异都正常编译 **9/11 exit101**，两次恢复cmp0。恢复后4 binaries52/0（旧broker21+新20+预算门7+late_response4）。真正settled未关闭执行由原API生成，无SQL伪造该状态；缺/坏ref、手工边和坏墓碑明确为gate fixture；拒读前后同一只读SQL快照比较全部表，已发生费用不回退。

原完整workspace1288/5五项均HTTP listener EPERM，R1获准相同完整命令复跑1293/0；默认CLI exit1只有readiness超时日志，不能单独证明stderr EPERM，环境重跑实际0。初本地git add index.lock沙箱拒写128，经同两白名单动作批准成功，非自动审批拒绝或外发。原日志全部保留。

卡 SHA256 `cbae1fd0ced666fa0e2cfef9ec154a3962fd5971416c3290e12ba7ba9dc91304`；R1环境 `38fa9f0f29ba66c11122c13f9f5a88fadbff0134df3675d24ad8890c81f2c66a`。产品返修0/环境裁决1。卡允许提前解析，故坏非redacted工件现在先校验再可能close，合法Rejected/Uncertain/redacted恢复保持旧行为；没有更改新provider-await结算路径或持久wire。

root首次记录脚本在提权默认python3落到旧解释器报SyntaxError，未写记录；同命令后段bash独立启动并完成本次fresh全量。已明确用out/pybin/python3补记，没有重复启动或把记录脚本错误算产品失败。

#### 子范围与边界

本地verified仅 E08/E03 的缓存读取：§11L1077、§11.3L1110、§11.5L1132–1134、E08L1299/E03L1229，V017/V018限定**本次只读事务快照可见的来源及完整上游撤销，阻止Completed缓存正文再读；保留费用和已发生历史**。缺ref不豁免；不修改账本、不退款、不伪造NotDispatched、不重派、不启动清理；公开历史receipt验证保持。

快照之后提交的撤销不在本次快照保证内；清理前仍可能有持久明文。无任意返回时刻并发线性化保证、不证明物理擦除/绝对脱敏/真实模型/完整typed E16 broker引用认证/完整恢复或V076/收益/发行。回滚参考实际父212c784，不回退账单、水位或审计。

admin未使用，无冲突。AG073遵从统一本地暂停从未请求外发，无本任务自动审批拒绝；AG071的精确公共推送拒绝及待用户问题仍有效。若main变化，须重新叠放并全新独立验收后才可合并。


- AG073全部root session最终0/已冻结本地验收后，仅回收target-ctrl-ag073，WT/base/branch/65份证据和全部QA保留，未合并。辅助chat已授权SC-IMPORT-CONTEXT gpt-6.1-sol/max，卡SHA32f639ef509c0cec7cd3e3c125b1384b0e3fc2494cf35561e9b98043c43ef7f4，只读核完整上下文/真实Skill消费者，不以固定模板冒充完整学习链。

#### 2026-10-01T18:55:44.668159+00:00 AG075 R1与074主控反例准备
- R1 SHA532744f534d6763b0741a58a22110a766cbc68e07ec2b2f42d67dedf1cd90a36明确正常objects CHECK(json_valid(body))，只区分合法JSON坏typed记录；SQL/底层解析保持原Err，不扩raw-body接口，不吞Internal。
- 074 root两个旧API反例已写未编译：已展开父的缺transition不能污染有效孙节点为OOS；JSON往返的多个历史兄弟节点不返还父机会，u32MAX公平等待与两槽/金额上限不能绕门。人工历史形状明确是夹具，不伪称旧程序产物。

#### 2026-10-01T18:57:28.526984+00:00 AG074 root第二反例夹具纠正
- 初父probe正常编译，P1实际[1,2,3]→OOS而非合法孙4为真实缺陷；P2在prefix.validate就因把Valid兄弟的best800k复制到HardFailure兄弟应继承父400k而失败，是root夹具设置错误，不算产品红灯。旧append/完整日志保存invalid-ancestor-fixture；只改新失败兄弟的best/gains继承父链，重跑实际父，两树最终用同一修正版。

#### 2026-10-01T18:58:34.573883+00:00 AG074 root实际父双反例完成
- 修正版append ac618b70409e1c2d755cf26b7569b494b43145aaa574c0e9e6dbaa0bb7de3062、source ac801442ed248371a5817f61a3c3a10d1473bc0b7433b354e0cedd1e3fabf352正常编译0/2exit101，P1非法旧父选中造成OOS，P2合法JSON往返前缀u32MAX等待使旧父45再次选中；不是prefix/compile错误，WTclean。

#### 2026-10-01T18:59:58.786704+00:00 AG074 root真实旧报告生成
- 在实际父212c用冻结旧API generator251e596dbb1a386c753adde7805fea25682273407c4feff09c77e6f6657da0c6执行生成1/0exit0，全部6文件create_new；QA/old-reports-ag074保留原件/provenance。三world与agent原产物逐字一致，三report语义digest/全部字段一致，仅真实测得replay_cpu_nanos和相应reports_body_digest不同；未重写旧产物。后续用这批root实际旧产物另在head执行升级读/重算反例，不把新构造报告冒充旧程序。

#### 2026-10-01T19:01:06.886174+00:00 AG074源码预审与第二只读侦察
- core+6/replay+7两个纯前缀集合guard及新core7test/newengine11test已完整阅读；三内嵌旧report与实际baseline capture逐字一致，root另实际重生成同语义。最终预审hash存qa/ag074-pre-review-hashes.json，尚未独立full/headprobe验收。
- SC-CURRICULUM-STATE-r4卡34471372c76fbbc24d4f5c74a1bd3d7f5ae467aa8e63819cc2a9ac5abcc418be，/root/impl_ag054 gpt-6.1-sol/max只读main，无Cargo/写入/下派；真源§8L938–946/§8.1L954–960/§8.4L982–986/E12L1351–55，调查历史失败与当前方向/已批准资产实际消费者，不发明退役阈值。与aux SC-IMPORT-CONTEXT共2侦察，实施074/075共2。

#### 2026-10-01T19:03:13.594573+00:00 AG074补充父旧报告控制与075进展
- root冻结source-root-old-reports.rs SHA917a11fd0741fdf5f6753c273a136672d5f5578ac2a3ccfe77087189f7cba0f7仅替换三旧report字节/hash为root实际生成，world原样。在实际父正常编译1/1exit101：旧affected verified本来Ok不满足新拒绝断言，chain/Recover控制通过。非编译红灯、事后clean，head额外2项待独立full后跑。
- AG075旧API两源missing-nonprimary-direct与wrong-kind-blob两事实正常编译2/0，仍不把新增API缺失当红灯；现实施六白名单。074正在完整self workspace，未结束不得提前求最终计数。

#### 2026-10-01T19:10:48.304187+00:00 续跑闸门与 AG074 环境裁决
- 启动提示词 b0000207…、第一真源70ec06e4…、v4.1归档45f3ba06…匹配；auth/fetch0，cargo1.98.1/Python3.12.14；main仍212c784，工作区clean。df Available26080000KiB=24.87GiB。
- AG074 R1 55dcc35a9352a8ad21dcb41f983063fe69a6af1081578d48a760a869c446a2cc：实际五HTTP bind EPERM，1286/5，原CLI仅readiness超时；同完整workspace已报告1291/0，root尚未独立验收，不提前标verified。
- 辅助chat SC-IMPORT-CONTEXT 完整报告 56870dc6b01a64e0d9b29df9f6a609928c403fd163f8c9e8b16e1def7349e257 已归档并完整读取；现有上下文字段没有实际tokenizer/request完整计量消费者、现runner不执行Skill正文、旧暂存run-only。后续须版本化真实内容驱动的本地fixture切片与typed来源，不能按计数填模板或伪造TrustedRun。无新增外部阻塞口径。

#### 2026-10-01T19:16:03.151543+00:00 课程侦察及导入下一切片草案
- SC-CURRICULUM-STATE child054/max已完成；主控读全部消息，保真事实归档 7d7e58d87030ff76e9d7b86e419b040a6b63617590cc551dd5c5a9747c116fad。现record_cycle/record_applied已实有，不能按旧GC6称完全缺失；资产仅摘要改变、无题目消费，failure永久优先但无方向/退役合同。两反例仍未执行。不得发明三轮退役或开sandbox disabled门。
- AG076-draft b68e5850…仅core三文件，真实完整fixture编码与内容驱动Skill执行内核；仍未派实施。辅助chat61sol/max做只读卡审，范围≤100行、不构建写入；与已完成课程侦察不超过2。无新持久容量/真实模型/正式批准范围。

#### 2026-10-01T19:17:36.092141+00:00 AG074 root接管与fresh独立验收开始
- 唯一head700308e4e8a546d21f1332c48ad28666e44c6d1c，父212c784，treeb2b00d82d9eadb77d9a6edbe62b3f45c05c2ef8f；四白名单+969/-0，13行产品与956行新测试hash同预审，clean。agent明确所有Cargo/双smoke结束不再写WT，最终REPORT仍只在scratch整理。原日志已归档qa/impl-ag074。
- 回收明确停止的target-ag-074及target-ctrl-base-ag074纯可重建缓存，为fresh full保20GiB；所有源码/WT/branch/反例/旧报告和原始QA均留存。root独立target-ctrl-ag074从不存在开始，jobs2，同ctrl_verify完整八门，不筛测试。

#### 2026-10-01T19:19:09.453916+00:00 typed预算只读侦察及075预审
- SC-TYPED-BUDGET派child054 gpt-6.1-sol/max，卡83cc11a940a9be27a98daec3912537cd1bf0c057a4c66e925dc288334b21499f，基线212c，只读主仓库，三问≤140行、不Cargo/写入/WT/下派。aux只读076卡审在途，总侦察2。
- root完整预读075新storage/engine产品初稿，提醒额外边不证明污染；实施方按原卡9把UnexpectedExistingEdge归Unknown保留码/记录。仍在实施未冻结，尚未验收。其首storage测FTS表ORDER BY rowid的snapshot夹具错已披露，不能算产品红灯；继续定向。

#### 2026-10-01T19:22:21.488504+00:00 AG075 主控反例冻结与父边界核验
- 冻结正常旧API source97d847304788bd3abece824d69853edc8c07f26313ca7377061208fc8de3faa1，root append3132fc075513f7afd2db58074aef7faec08a0b27f28e8c4733bd9aff4a9517c7。实际父212c独立WT/target正常编译，最终结果见ctrl-ag075-adversarial-base-212c784.log；不把没有新API当编译红灯。
- P1同idforeign边+300额外诊断边不修本namespace直接孔洞；旧load仍成功但依赖查询精确区分；P2篡改仅兄弟owner后旧load拒绝、主来源原样；所有表只读快照不变。新API额外Q1小预算中页不得报missing、穷尽后找非primary缺边；Q2source-only不读无关foreign兄弟而result/selection必须拒绝。head额外尚未运行，SHA207d1c04ec489e0c30fd35dd215f9d2a8c94e3415c3ae26ad8e02f9050ef9e83。

#### 2026-10-01T19:22:58.115395+00:00 AG074 fresh全量完成
- ctrl-ag074-700308e.log八门exit0，root逐行独立求和98组=93测试二进制+5doc，1291/0=实际父1273+18新测，Python48。session81490最终结束，接着只在此稳定head运行已冻结双probe及独立实际旧程序报告两项检查。075共同父probe实际2/0exit0/clean，是边界控制，不声称旧缺陷红灯。

#### 2026-10-01T19:25:09.211604+00:00 AG074 head反例与旧报告兼容检查完成
- ctrl-ag074-adversarial-head-700308e.log同冻结source/append正常编译2/0exit0，实际父此前0/2exit101；P1缺transition非法重复父不再抢合法孙节点，P2合法历史多兄弟/u32MAX等待不返还父机会。事后clean。
- root真实旧程序报告冻结source917a11fd…在head补充2/0exit0，actual父1/1：affected新拒绝而普通chain/Recover仍verified/idempotent；原6 JSON hash待验收清单统一核。所有root session已最终结束。最终实施REPORT已全读，待归档manifest完整性验证后落本地验收结论。

#### 2026-10-01T19:29:13.955499+00:00 派发 AG076 与075全量资源许可
- AG076 E16.1/E02/E03、V059/V089限定完整fixture编码与旧编辑器、V076仅内容因果的内存前置。卡cards/AG-076.md SHAda488e2bf452dc52035be6ab697c1bca994906628a0c77862872bed08bad2e9f；基线212c78488b403397eaebaa65a6d280b9c6be3899，WT out/work-20260930/ag-076，branch wrokbot/ag-076-import-fixture-content，target-ag-076，/root/impl_ag057 gpt-6.1-sol/max正式派发。白名单core lib单行+新module/test，先静态，Cargo另协调。当前实施075/076=2，侦察typed预算=1；aux卡审已完成。
- 正式卡采纳只读审核8项：可信control不从JSON自授、全材料闭包等集、artifact/locator摘要分开、同计量输出经旧parser、保护区/4096整替换拒绝、明确字节/案例/来源caps及有界序列化、具区分力反例、本次ProgramFixture与历史UnverifiedImport分开。无新生产任务/持久容量授权。
- root074与075父所有Cargo已结束，清其停止purecache、QA/WT/branch完整；075变异后26/0获准jobs2同完整clippy/full/build/smoke，每大构建df≥20GiB。不外发。

#### 2026-10-01T19:31:44.258038+00:00 CTRL-AG074-R1 本地独立验收完成
- 清单qa/ctrl-ag074-local-acceptance.json SHAfb782db58200b4a4d5f3694513bac47e6055bcb9a0b75ecee10c04f7741a8ddc；完整验收记录如下，无PR/未合并。

# CTRL-AG074-R1：正常 Deepen 共享已观察父展开规则，本地独立验收通过

固定head `700308e4e8a546d21f1332c48ad28666e44c6d1c`，父/当前main `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `b2b00d82d9eadb77d9a6edbe62b3f45c05c2ef8f`。唯一提交，无叠放/冲突，交付及所有root探针后clean。四文件+969/-0：core strategy+6、engine replay+7、新core测试255、新engine测试701；全部内容此前完整预审，最终四SHA逐一匹配。无PR、未push/merge，不计已合并产品数。

两个产品门从已观察prefix构造父集合，仅过滤已有任意子节点的正常Deepen，回放available与core决定同步，不窥未来结果、不改Recover。按既有在线exploration规则和真源共享合法性目的，把同观察前缀每正常父只消费一次提升到core/replay；真源没有逐字规定跨批每父一次，明确属主控工程推导。合法Valid子可继续加深；OOS/Censored未形成子节点不预先夺走机会。

最终产品hash：core `0d257897604e279b69c13ffe76a6fa4bc89da3574726bf125cdfd688bd65542b`；engine `159a5798c45e5cdca10973607aa28bfef2327c6a5af8614be78a6cb8130769c9`。测试hash：core `7d36b078e7701aa53d50a87456007b3589c98f31e5c1b8ead2c4e0a561335a6d`；engine `2a08a243364574490fb1dc2801c53a2b87a5b5d7d037e03c1e58adc569be2f7e`。

#### 独立完整复跑

全新target-ctrl-ag074，`ctrl-ag074-700308e.log*`八门实际exit0：fmt/clippy/workspace/build rsia/两smoke/support checker/Python unittest。root逐行独立求和98结果组=93测试二进制+5doc，**1291 passed / 0 failed / 0 ignored**，与父1273+新增18相符；Python48。jobs2只控制资源。新增18内有可选旧报告生成入口默认早返，不能把默认全量这一项当真实旧程序产物证据；独立生成见下。

#### 主控反例与实际父对照

冻结旧API源 `probe-ag074/source-replay-v41-212c784.rs` SHA `ac801442ed248371a5817f61a3c3a10d1473bc0b7433b354e0cedd1e3fabf352`，append SHA `ac618b70409e1c2d755cf26b7569b494b43145aaa574c0e9e6dbaa0bb7de3062`。同一代码两树正常编译：head `ctrl-ag074-adversarial-head-700308e.log` **2/0 exit0**；实际父 `ctrl-ag074-adversarial-base-212c784.log` **0/2 exit101**，均事后clean。

- P1真实sealed world有root900k→child200k；旧父另seq3故意没有transition，合法孙seq4有950k。W=1/2/4必须选[1,2,4]、cost3、无OOS、无非法父等待。实际父选[1,2,3]导致OOS，head全部正向通过；不是通过窥未来剔除缺transition。
- P2由真实两节点回放构成合法JSON往返前缀，再加入明确人工历史HardFailure兄弟，旧父45等待u32MAX。不同W及action顺序仍只能选合法子50/其他根60，费用/槽位正确、原prefix逐字不变。实际父选择45。人工兄弟是历史形状反例，不冒称真实新执行。

初版P2错误复制Valid子best/gains到HardFailure兄弟，prefix.validate失败是root夹具错；原append与完整日志保留invalid-ancestor-fixture，不算产品红灯。修正继承父链后上述最终父0/2为真正选择断言失败。

#### 真实旧报告兼容补充

root在实际父212c运行冻结旧API generator SHA `251e596dbb1a386c753adde7805fea25682273407c4feff09c77e6f6657da0c6`，`ctrl-ag074-old-generator-212c784.log` **1/0 exit0**，create_new生成三world/三report。原件在 `old-reports-ag074/`，provenance列六hash；本次全部再次核原样。三world与实施方捕获逐字一致；report仅CPU与对应body_digest不同，其他字段/semantic_digest一致。

root真实旧report hash分别：affected `e566ece73a28d32f40b8c3d87b89ac40861e12ccdad7d1bfdffaec639e62412c`，普通chain `68cba2c5a730c9c7f338f4ce721ebd763f3af7bc2f5cc68333482d224d6966fe`，Recover `ff6510f0c222358bf755b6b5a14a539d1f7189da914b40ee4680637648611838`。

补充冻结 `probe-ag074/source-root-old-reports.rs` SHA `917a11fd0741fdf5f6753c273a136672d5f5578ac2a3ccfe77087189f7cba0f7`，仅把三个内嵌report字节/hash换成root实际旧产物，world不变。同一源码实际父 `ctrl-ag074-root-old-reports-parent-212c784.log` **1/1 exit101**（受影响旧report旧验证Ok不满足新拒绝断言，两个不受影响控制通过）；head `ctrl-ag074-root-old-reports-head-700308e.log` **2/0 exit0**。static load仍读原完整artifact，受影响verified/重复同ID重算拒绝并保持原payload，普通链/Recover保持verified/idempotent及旧CPU。原程序产物来自synthetic worlds，不是模型效果或实际修复证据。

具体轨迹自然变化：受影响旧[1,2,3]/BudgetExhausted/complete/950000/有score，变成[1,2]/WorldExhausted/incomplete/900000/None。未改终态/评分公式、wire/digest/engine版本；不兼容旧report同ID重算失败关闭，是明确行为兼容边界，不能宣称自动迁移。

#### 实施证据及结论

`impl-ag074/REPORT.md` SHA `0d7e5be11ececbc9dffe00a5cd328837fba0ff0d596da02d0db2e699908b9b44` 和20项 `command-results.json` 全部已读；41项manifest SHA `5f484241862c0c1829c46445c804a5fc60cf304a19ac3dda05bb9db07348cf56` 逐条实际大小/hash核对归档，包括六旧JSON。未过滤原失败。

自测最终core基线0/2、engine1/3均正常编译；新core7/0、engine11/0。删core门变异3/4、只删replay available门变异0/1抓公开waits泄漏，两次恢复cmp0，旧回归core29/0、engine79/0。原全量1286/5仅五HTTP bind EPERM；R1同完整workspace1291/0。原CLI仅readiness超时，不能凭其日志称stderr EPERM；相同环境复跑双smoke0。没有外发自动审批请求或本任务工具拒绝。

卡 `ee6487be89271262896ef9f63869c23a25b46d86512260462a47838cad72a6f6`；R1环境 `55dcc35a9352a8ad21dcb41f983063fe69a6af1081578d48a760a869c446a2cc`。产品返修0、环境裁决1，旧测试零改动。

本地verified仅E09/E10正常Deepen已观察前缀合法性：§7.1L785、§7.1.1L791–793/797/805/807、E10L1321–1327；V020/V022/V023/V025/V081/V086的有限本地子范围。不声明完整available_actions、一般PolicyStop/WorldExhausted/complete_support合同、生产Recover来源/修复能力、旧报告迁移覆盖、世界版本升级、完整E09/E10、真实模型/收益/发行。

回滚参考实际父212c，保留已有世界/报告/费用/撤销事实。无admin使用。AG071精确公共push两次自动审批拒绝和待用户授权仍挂起；AG074依主控要求从未请求外发，不能归因其本身工具拒绝。若main前进，必须重新叠放并全新独立验收后才可合并。验收结束已回收停止的纯target，所有QA/WT/branch/真实旧报告保持。


#### 2026-10-01T19:43:08.248140+00:00 续跑闸门、AG075环境裁决与磁盘暂停
- 提示词b0000207/真源70ec06e4/归档45f3ba06核对匹配；auth/fetch0，cargo1.98.1/Python3.12.14，main/origin仍212c784，clean。df初17,760,000KiB（16.94GiB），稍后18,800,000KiB（17.93GiB）；已通知075/076停止新Cargo，075确认唯一workspace session75420结束。只发现target-ag-075缓存2,853,224KiB，无其他可回收target；未删除非缓存数据。ps在默认沙箱被拒，不据此猜测运行状态。
- AG075原完整1296/5=1301（93bin+5doc），主控读五处HTTP bind EPERM；R2 d5fafcbe2c826ab4875b04166e82f2f8dca72d771ebf401f70b6adac1b4c0458允许同源码完整环境复跑，但资源许可尚未恢复。原scratch证据归档qa/impl-ag075，源/日志/WT完整。

#### 2026-10-01T19:45:10.190495+00:00 停止缓存回收
- AG075确认全部Cargo/session结束，076从未建立target；原075证据56项已归档。主控仅删除2,853,224KiB的可重建target-ag-075，沿用此前未验任务空间裁决。源/WT/分支/QA不动，不能把缓存删除算已完成验收。后续075需重新构建同一冻结树。
- 回收后可用 22159360000 bytes = 20.638 GiB，未核够20GiB前不恢复构建。

#### 2026-10-01T19:50:06.385645+00:00 AG075静态全读与077预算草案
- AG075六文件最终1124行engine/121行storage/1360行engine测试/253行storage测试及两lib各一行全部完整读审，hash与pre-workspace-freeze逐项一致，冻结qa/ag075-pre-review-hashes.json。未发现需产品返修的具体偏差，仍待全量/root独立反例，未标验收。
- SC-TYPED-BUDGET全消息已读，保真整理报告55ad9ccf74700ca2e082f345395181412404591659b97bf43214efe83a5b1389；新预算草案AG-077-draft SHA37654bc673cdd4552900bd165217bb3841c06ed72481f05ea3862c8b2a7f37c3未冻结/未实施/无WT。预算物理body摘要与业务typed摘要明确分开，新的safe消费/结算兼容须卡审后定。
- 只读卡审交child054 gpt-6.1-sol/max，仅原main+草案，<=120行，无构建/写入/其他WT/下派；实施仍075/076两路，076静态，唯一Cargo075同冻结树R2full。

#### 2026-10-01T19:51:42.526489+00:00 AG075完整环境复跑通过及缓存分段回收
- R2同完整workspace 1301/0（93bin+5doc），exit0，session81419最终结束；主控读原json/log，无过滤，六文件冻结不变。所有新证据再归档。df19,000,000KiB，93测试exe仅1,560,812KiB，不足恢复20GiB；因此回收全部已停止target-ag-075。工作区/证据不变，不重复已通过全量。
- 回收后 20.294GiB，仅许可同树build rsia；完成后保留rsia二进制、回收其余纯缓存，再在>=20GiB下跑smoke。避免并行重建，076继续静态。

#### 2026-10-01T19:56:51.390901+00:00 AG075 build通过、保留执行件回收缓存
- 唯一build session79169已结束/closed，build-rsia exit0/56.126s；root核debug/rsia SHA256 244548acef37829af68214cdceb9dd4b5ad29b366a1c8a3c5036f4f733bc4cb8。只保留该路径可执行文件，删除其余停止的target编译缓存，原QA源码不变，事后hash一致。可用20.065GiB，下一步原双smoke，076仍等待单独许可。

#### 2026-10-01T19:57:20.741396+00:00 AG076独立旧API边界反例准备
- 冻结旧API source cc6c26bc1594f1194ee7e493f6e13d9eb9bc94bd6477bfec159245f6d1b179b1，root append f786f1748827655d3da9bf345789dbcd0c4df1d3bf72a70b96638bf06568e631，均尚未编译/执行。P1三个32KiB NUL part实际JSON远超literal字节，旧per-part门不保证完整上下文；P2旧编辑器allowed集合是允许子集，不证明必须保留所有未选/反例源。两树应保持这些旧事实，后续新API要另验严格完整预算/闭包，不能冒称旧缺陷红转绿。

#### 2026-10-01T20:07:34.942723+00:00 AG075本地交付与单路资源协调
- 子代理065确认所有smoke/session停止，head3d4995a4c0e122a409908aca21c512fc46372dce/parent212c784/tree3da9ea4f747931c2881a4288b06eba86ded80402，sixfiles+2860/0、1301/0、双smoke0，仅本地未验收。CLI原readiness1按R2同cmd环境复跑0。
- 主控回收已停止且不再使用的最后075 debug/rsia缓存（核hash244548ac…）；所有QA/源码/分支保持。回收后可用 21504000000 bytes = 20.027 GiB。076旧API no-run0 session49376已结束，因磁盘少11520KiB尚未实际run，不冒称基线已跑。

#### 2026-10-01T20:08:21.036202+00:00 AG075交付核验
- final-report全文和完整28项命令表已读；79项evidence-index实际逐条大小/hash匹配，全部归档qa/impl-ag075，源码六hash与主控先前全读冻结逐项一致，working tree clean，head3d4995a4c0e122a409908aca21c512fc46372dce。report ce116045ffa15d586d93ceaf5df503908f4a4c10effebfe358e0dc827bb54909，manifest04338dcfd397d561d08c0cdf129bdc88bdca71cf363d76c20596df27d2555913，indexffe84d10d9f5509990d8422495549de78769c88c80e5c34ab578ab86a7a28185。等待076基线结束回收其停止缓存后开始root全新复跑。

#### 2026-10-01T20:15:48.441943+00:00 AG077正式裁决冻结、资源暂停派发
- child054 SC-AG077-DRAFT最终全文已读；主控保真整理report 3174fab22765c17aebcc76f3d1791eb99e54bcc48d66d55870408428a81f3991。实际旁路为registered settle及共享repeated settle两正文结构；owner原Admin不同Host、raw SQL摘要不同typed fingerprint；redacted扫描缺ref必须提交Failed而非仅rollback；Complete队列不重开。
- 正式卡AG-077.md SHA fed505d275af29989747e1d9584392ab52acae5b5941b85414b1ecd8978c6de5，E04/E08/E16.1/E16.5，V008/17/18/38/94/96有限子范围，V076仍pending；newrequest/ref独立schema，三正文脱敏而费用保留，恢复双端schema保护，新engine真实writer测试白名单。尚未派发、无WT/branch/target，因当前20,080,000KiB低于20GiB而停止新派发/构建，不碰不相干数据腾空间。
- AG076 R1资源裁决 1d04d0ef83b9e354196d300398714c4b3ba8b36283c4f27428b60c58a1ad7c05，仅准已编译2.3MB的纯内存两测直接执行，先核原binary/sourcehash；no-run为正常Cargo编译0，原run资源检查98非测试结果。完整必跑和root fresh未减。其余任务无新外发申请。

#### 2026-10-01T20:16:16.231878+00:00 AG076原编译产物实际基线两测
- 子代理按R1直接原binary --nocapture，session70654关闭，实际2/0exit0：完整请求263227bytes旧门接受；旧编译器接受两个操作顺序不同正文但不执行行为判断。未启动Cargo/no重编译；原资源退出98独立保留。基线日志/hash/命令归档qa/impl-ag076-baseline。
- 已获全部target访问结束确认，主控回收其103536KiB可重建target，源码/QA保持。回收后19.226GiB，仍不足20GiB，root075 fresh及077派发继续挂资源；076仅允许已授权静态三文件实施，不标验证。

#### 2026-10-01T20:18:35.838930+00:00 AG076静态全读
- root完整读875行模块与1473行32新测试及冻结2旧基线；产品模块与scratch草稿hash相同、新测试逐字为原2+32，lib仅module。独立Python对实际tool/output schema常量JSON解析通过，但不代表Rust编译/测试。qa/ag076-static-review.json记录三hash；没有发现需扩卡的具体静态偏差。任何新能力、变异、full均尚未跑。

#### 2026-10-01T20:20:01.475110+00:00 AG076主控新API反例准备（未运行）
- qa/probe-ag076/head-new-api.rs SHA 63f8ee932d4376cfb3bec8499d9c5ca30d0b1b95e86fae9bcdaa293138037a6c。Q3固定非对称输入的六种操作排列手算输出，经真正编译再JSON往返父及第二轮no_change，行为/历史轴/计数-only依赖均保留；Q4完整重算正确hash/token数后注入unicode转义同名重复key于control/方法/父，须解析固定拒绝且不污染正例。head only新能力验证，不能把父缺新API算失败。当前磁盘19.23GiB，均未编译/运行。

#### 2026-10-01T20:20:50.765724+00:00 AG075主控fresh全量启动
- 当前可用22.163GiB，恢复资源许可；六hash/head/parent==origin/main/工作区clean全部实核，target-ctrl-ag075此前不存在。主控单路jobs2跑固定3d4995a全量八门，子代理076仍无Cargo。

#### 2026-10-01T20:21:47.953302+00:00 AG077正式卡只读复核
- child054 gpt-6.1-sol/max复核正式卡fed505d2…，仅新矛盾/共享出口/cleanup真实旁路与白名单，<=80行，禁止写入/构建/其他WT/下派；无实施派发。075主控全量session11947运行，076静态，075/077 storage/lib同文件故等075验收结束再派。

#### 2026-10-01T20:26:33.438386+00:00 AG075主控全量八门通过，探针前空间回收
- session11947实际结束exit0，八门均0，独立求和98组=93bin+5doc，1301/0/0ignored，Python48。源码clean/固定head不变。当前19.798GiB，因此仅按原full日志精确93路径回收已停止的测试执行缓存，保留同head依赖编译缓存供反例；详qa/ag075-stopped-test-cache-reclaimed.json。回收后21.286GiB，所有QA/源码/WT保持。


#### CTRL-AG075-R1 / 本地独立验收（未建PR、未合并） 2026-10-01T20:27:10.779214+00:00

固定 head `3d4995a4c0e122a409908aca21c512fc46372dce`，实际父 `212c78488b403397eaebaa65a6d280b9c6be3899`=当前origin/main，tree `3da9ea4f747931c2881a4288b06eba86ded80402`。无叠放；六白名单文件+2860/-0，两lib仅pub mod，旧测试/依赖/锁/迁移未改。主控此前完整逐行读全部产品与新测试；最终六hash与预审冻结及交付manifest一致，工作树clean。

##### 独立复跑与反例

全新target-ctrl-ag075，`qa/ctrl-ag075-3d4995a.log*`八门实际exit0：fmt、clippy、完整workspace、build rsia、双smoke、scope检查器、Python unittest48。独立逐行求和98组=93测试bin+5doc，1301 passed / 0 failed / 0 ignored，等于父1273+28新增。jobs2仅资源调度，未过滤测试。

冻结旧API源 `qa/probe-ag075/source-old-api-212c784.rs` SHA97d847304788bd3abece824d69853edc8c07f26313ca7377061208fc8de3faa1，共同append SHA3132fc075513f7afd2db58074aef7faec08a0b27f28e8c4733bd9aff4a9517c7；新APIhead额外 SHA207d1c04ec489e0c30fd35dd215f9d2a8c94e3415c3ae26ad8e02f9050ef9e83，合并append-head SHAc5ad3ff95eece26d50400b451a3a5dc2f60c84f02c48091904b9c23f2452c202。

实际父 `ctrl-ag075-adversarial-base-212c784.log` 正常编译2/0exit0；head `ctrl-ag075-adversarial-head-3d4995a.log` 正常编译4/0exit0；两树探针后clean。共同两例是诚实边界核验，并非红转绿；新API只在head存在，缺符号编译错误没有用于基线证据。

- P1真实writer产物复制后缺result→非primary直接边；传递路径仍有它，foreign namespace同名边和300额外边不能补齐自身缺边；旧load成功事实不变。
- P2仅第二source换成foreign owner，旧result load拒绝且primary原样；全SQLite规范快照前后相同。
- Q1 page_size=1且行预算0/1/2/3/256/257均Partial，不在未穷尽时猜缺边，lookahead实际读取也不超过预算；预算10000、页宽1/17/256均穷尽302边，精确定位1条缺非primary边与300条额外unknown边，不让foreign同名边掩盖问题；报告physical_blob_check=not_checked。
- Q2 source-only仅读自己及selection backlink两对象，不读取/背书foreign sibling，selection不展开边；result/selection完整检查必须Forbidden。两类head诊断全部SQLite表前后相同。

##### 实施证据及裁决

交付 `qa/impl-ag075/final-report.md` SHAce116045ffa15d586d93ceaf5df503908f4a4c10effebfe358e0dc827bb54909，manifest04338dcfd397d561d08c0cdf129bdc88bdca71cf363d76c20596df27d2555913，79项evidence-index ffe84d10d9f5509990d8422495549de78769c88c80e5c34ab578ab86a7a28185已逐项实际大小/hash归档；报告全文和28项命令表已读。

自测旧API no-run0、真实两例2/0，最终storage4+engine24=28/0；省非primary边变异最终正常编译后18/6、去namespace变异3/1，恢复cmp0。原FTS rowid夹具错、Option编译错、两clippy lint均留原日志，不算产品红灯。原workspace1296/5五个HTTP bind EPERM，R2同完整命令1301/0；原CLI只证readiness超时exit1，不冒称该文本证明stderr EPERM，同cmd同binary环境复跑0。主控此次独立八门全过。

卡SHAf3a6621e090f0a04810b9a2f63f61e2179bb325e02852237e24611a237befe46；R1 532744f534d6763b0741a58a22110a766cbc68e07ec2b2f42d67dedf1cd90a36，R2 d5fafcbe2c826ab4875b04166e82f2f8dca72d771ebf401f70b6adac1b4c0458。R1坏JSON口径以正常SQLite CHECK为准，合法JSON typed坏字段分类，底层SQL/JSON失败原Err；R2仅监听环境复跑。未要求产品返修；实施期clippy整改属原白名单。

##### 结论与未完成范围

本地verified仅E08有界typed E16声明/直接边只读诊断：E08L1295/1299、§6.3L644/648/650、§11L1077/1081/1110/1112/1132–1134，V017/V018/V056/V075有限诊断切片，V076仅前置。一个Session快照、最多202对象点读/10000实际边行/256页宽；不穷尽必Partial。已知typed fingerprint与Value重排分开；合法prepared/failed/进行中摘要转变保留Unknown/合法状态；redacted仅History，额外边Unknown，不能当已证明污染。

不可声明全部namespace/所有E08对象完整性、业务授权/当前live消费门、物理blob/文件检查、绝对正文内存CPU上界、数据库修复、完整三reader学习新使用撤销链、真实模型/收益/发行。source-only的selection只是归属回指权威，不扩大为其全部兄弟闭包。namespace水位是诊断快照，不能当返回时刻授权。没有CLI/自动修复/持久化报告接线。

未建PR、未push/merge、无admin，不计合并产品或support-scope已合并记录。AG071精确公共push自动审批拒绝及待人类授权仍挂起；AG075自身未申请外发、没有自动审批拒绝，不混写。若main前进须重叠放且fresh全量和反例复跑。回滚参考父212c；保留既有来源、费用、水位和历史事实。全部QA/WT/branch保留，仅可回收已停止纯编译缓存。

#### 2026-10-01T20:27:46.490941+00:00 AG075验收后缓存回收
- root full11947/probe30189均实际结束0；验收manifest8719a09ae2b3a14789d71d3e7a59e7983f3d9645f7ec84eff97d460d1d8e25c4、记录/PRbody/ledger/二验未合并附录齐全。仅回收停止target-ctrl-ag075，QA/WT/branch/base保留。可用22.202GiB。076获下一单路定向/变异构建窗口，full另协调；077尚待正式卡复核结论。

#### 2026-10-01T20:30:22.874344+00:00 AG076主控父反例夹具纠正
- root实际父session66329正常编译后1/1exit101：P1使用NUL被既有text()门合法拒绝，是主控夹具前提错，不是产品缺陷，也不算新能力基线红灯。P2通过。原append/combined/log均以initial-nul-fixture保留。
- P1改为旧门允许且JSON同样六字节转义的U+0001，每part32KiB；其余断言不变。新append 543a06606149f703316c1d1faa9df2582683dfa0f92d0f2160d2498b6b500124、combined db3c8845e15e512323e5a0dac4e3df534e56b0c439dae02ebcea90b04fb53401，正在同真实父正常执行两测。

#### 2026-10-01T20:30:42.241915+00:00 AG076修正后实际父反例通过
- ctrl-ag076-adversarial-base-212c784.log正常编译2/0exit0，git事后clean。U+0001三part literal98304、实际JSON590948bytes，旧subset允许省required源为已证边界；不称红转绿。所有root session已结束，纯target-ctrl-base-ag076回收，base WT/QA保留。
- 上行actual JSON数字笔误更正：原日志实际为 **590957 bytes**，不是590948；原日志/断言不变。

#### 2026-10-01T20:44:45.353227+00:00 AG076 环境裁决 R2
- 卡AG-076-R2.md SHA 957fece09cb7f0752a164899430452882a85329daf2331d4af54934d27b4300a；原完整1302/5仅五HTTP bind EPERM，原日志已读并独立求和97组92bin5doc；三源冻结不改。恢复后58/0、clippy0、build0；CLI原readiness1保留，准同完整workspace/同CLI请求本地监听环境复跑、jobs2、每次df>=20GiB，无测试过滤或产品修改。077静态准备，不占第二构建路。

#### 2026-10-01T20:45:32.198034+00:00 AG077 派发前 R1 最终裁决
- 正式卡fed505d275af29989747e1d9584392ab52acae5b5941b85414b1ecd8978c6de5不变。R1 SHA 7151bc6d6ea9459bf16696751221e2caab194a78f2113eb4ad7ad234912feb4c；派发前原稿98110124f4e0e0aad607f7659a6a0e48b0c8cacf540c779761ff3fd1096e15cf另存before-late-fixture-clarification，唯一新增是迟到夹具须真实预算API、不假定撤销后reserve成功。尚未派实施，无产品返修。
- R1依据§11L1077/1110/1112/1132–34、E08L1295/1299，明确重复settle先samecharge/fresh来源再正文幂等，间接ref→S→R传播必须脱敏三正文、历史索引先于direct过滤及实际progress。child054最终只读报告保真存SC-AG077-FINAL-r4-report.md SHA 8f6bddfe389888ee532104b02d81182e2a7b1a010706adcafdbe45d44bfae9a4；无测试成功声明。

#### 2026-10-01T20:46:15.850509+00:00 AG077 正式派发
- E04/E08/E16.1/E16.5，V008/V017/V018/V038/V094/V096有限预算/撤销/恢复，V076前置仍未完成。卡cards/AG-077.md SHA fed505d275af29989747e1d9584392ab52acae5b5941b85414b1ecd8978c6de5，R1 SHA7151bc6d6ea9459bf16696751221e2caab194a78f2113eb4ad7ad234912feb4c；_common cf21103badabebda012c1b92883d446e65a25a616bfbe376909d570726cc77b2。
- /root/impl_ag065，实际 gpt-6.1-sol / max（L及安全恢复最高档）；基线当场origin/main212c78488b403397eaebaa65a6d280b9c6be3899；WT /Users/wrok/Desktop/RSIAgent/out/work-20260930/ag-077，branch wrokbot/ag-077-e16-budget-bindings，target独立target-ag-077，scratch ag-077-impl。
- df21440000KiB≈20.447GiB。075已结束，同storage/lib文件串行满足；076仅core互不重叠，当前实施2。077仅静态设计/自身基线准备，产品diff0，未许Cargo；076唯一构建。无外发、本地交付。

#### 2026-10-01T20:47:35.985651+00:00 完整导入链下一消费者只读侦察
- SC-E16-NEXT-CONSUMER卡SHA 068c4855a3112e33aa1c95c0bc503b870635967ca57229fd7d4d8a0fd51f80fe，child054 gpt-6.1-sol/max；仅main公共Candidate/Pattern/路由/ProgramFixture正式拒绝和真实后续使用接口，<=140行。无实施/构建，明确070/076/077未merge不能当主线API；真源§6.3/6.4/11/E16.1/V076。

#### 2026-10-01T20:50:17.042957+00:00 AG077 root旧API反例冻结
- source-budget-revoke-212c784.rs SHAea4f2cb3e3b7bbe387d4f2b04a49d6a4450a06b52f18ed94a5d1e0e7dd738304；append.rs SHA2a4c6bbbb4651ad9f8704dfdf28c51345061f985af61fc9c15a4453e14aa033b，尚未编译/运行。P1旧run-only门不查间接artifact上游；P2旧首次settle后非primary撤销、重复settle仍返旧response且费用/全DB不变。明确两项是原API边界，两树应保持；新模式另写head反例，不能冒充旧API红转绿。

#### 2026-10-01T20:51:35.117600+00:00 AG076自测完成与停止cache回收
- child确认全部session实际关闭、不再访问target；root归档48项现有证据到qa/impl-ag076，独立sum完整R2为1307/0/97组92bin5doc；双smoke0，三源码cmp0。仅删除停止target-ag-076约2657892KiB可重建cache，源码/WT/全部QA保持。回收后24576000000 bytes=22.888GiB。等待本地提交与最终报告，尚未root验收。

#### 2026-10-01T20:54:06.909901+00:00 AG076主控fresh全量开始
- fixedhead e706719af6b1b59a6b92329a2ea87fdd588d20a7/parent==origin212c/clean，三最终源码hash与全文预审匹配，fresh target-ctrl-ag076此前不存在，df22.454GiB。root单路jobs2八门，077旧API正常编译+实际2/0已结束，仅静态产品。

#### 2026-10-01T20:55:00.768104+00:00 AG076主控head探针派前断言增强
- 新API probe首轮尚未运行；增加实际cases.len==2与首轮正文确有改变，避免空案例使all/for断言空过。旧head-new-api63f8ee93与combineddb3c8845原件留before-nonempty-assertions。新head额外SHA11d339951e965b2d3afa9a5e8d2ad2d33a2ff70f2733c16e33f3dca9bf569883、combinedSHA315dc505c182d3d58d80efdba2f0602e442fb6fed65156d46bd8c225246daa06，旧API source/commonappend保持，实际父2/0证据不受影响。

#### 2026-10-01T20:56:34.213844+00:00 GA26未决费用只读侦察
- 用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc gpt-6.1-sol/max；SC-BUDGET-INSPECT-r4.md SHA06736772acf494e741406c14b7bc6a19ccb0287a1a69f1cab782a22d8e171f84，仅main预算状态/字段与现有CLI/inspect、离线报告可行性事实，<=100行；无构建/写入/实施。依据真源§3.5/3.6、L705/709/761/1077/1112/1134、E16.5。两scout上限内。

#### 2026-10-01T21:00:34.393243+00:00 AG076主控全量通过、反例前回收停止执行缓存
- root session97044实际结束exit0；八门全0，独立求和1307/0/97组92bin5doc/Python48。可用20800000KiB低于20GiB，因此仅按本次实际test日志精确路径回收92个已停止测试exe（详qa/ag076-stopped-test-cache-reclaimed.json），其余同head依赖缓存供probe。QA/源码/WT不动。回收后21.304GiB，准四head反例。

#### 2026-10-01T21:01:35.693458+00:00 AG076主控反例完成
- root session43590实际结束0，四head probe正常编译4/0，source/common与实际父2/0边界原样；combined315dc505…，六排列/二轮no_change及Unicode重名key拒绝均通过。探针后WT clean/head不变。只回收停止target-ctrl-ag076剩余cache，QA/源码/WT/branch/base保持，可用22.095GiB。等待实施最终索引冻结后写正式本地验收记录，尚无PR/merge。

#### 2026-10-01T21:02:49.567958+00:00 完整导入链消费者事实复核
- SC-E16-NEXT-CONSUMER完整消息已读，保真报告SHAbedeb06073a234857c1ed27dbf225428774b75792ca9f5a7df4be66f8e5bee88。确认main已有可信run候选/巩固持久链及ProgramFixture报告，正式approve真实拒绝；现runner不读Skill正文，E16 Pattern/非Skill路由无持久消费者。070/076/077未merge，不以卡冒充API。后续正文执行/开发新使用必须独立授权/来源闭包，不伪造AppliedReceipt或绕Active门；资产容量及管理合同不自造。仍有可独立diagnostic子范围待卡裁决。

#### 2026-10-01T21:03:32.418344+00:00 AG077 root实际父边界对照
- session92307正常编译后2/0exit0，事后clean；旧run-only不查间接artifact上游及旧repeated settle不fresh撤销两事实明确边界，不算红转绿。sourceea4f2cb3…/append2a4c6bbb…不变。回收停止纯target-ctrl-base-ag077，QA/base保持，余22.087GiB。root当前无Cargo，077可单路定向/变异jobs2，full另协调。


#### CTRL-AG076-R1 / 本地独立验收（未建 PR、未合并） 2026-10-01T21:05:24.512154+00:00

固定 head `e706719af6b1b59a6b92329a2ea87fdd588d20a7`，实际父及当前origin/main `212c78488b403397eaebaa65a6d280b9c6be3899`，tree `80cc17c2ff9d39d83b08305ba3832cc126cc5fe6`。唯一提交、无叠放/冲突，三白名单文件+2492/-0：875行新模块、lib一行、1616行新测试（2个旧API事实+32个新能力测试）。全部产品/测试已完整逐行预审，最终hash与冻结一致，lib仅fmt排序后的单行pub mod；工作树clean。无旧测试、依赖、锁或迁移改动。

##### 独立复跑与反例

全新target-ctrl-ag076，`qa/ctrl-ag076-e706719.log*`八门实际0：fmt、clippy -D warnings、完整workspace、build rsia、双smoke、scope检查器、48 Python unittest。root逐行求和**1307/0/0ignored，97结果组=92测试bin+5doc**，等于父1273+34；jobs2只调资源，不过滤测试。

共同旧API source SHA `cc6c26bc1594f1194ee7e493f6e13d9eb9bc94bd6477bfec159245f6d1b179b1`，commonappend SHA `543a06606149f703316c1d1faa9df2582683dfa0f92d0f2160d2498b6b500124`。新API额外 SHA `11d339951e965b2d3afa9a5e8d2ad2d33a2ff70f2733c16e33f3dca9bf569883`，combined SHA `315dc505c182d3d58d80efdba2f0602e442fb6fed65156d46bd8c225246daa06`。

实际父 `ctrl-ag076-adversarial-base-212c784.log` 正常编译**2/0 exit0**；head `ctrl-ag076-adversarial-head-e706719.log` 正常编译**4/0 exit0**，两树事后clean。共同两例是边界核验，不能声称红转绿；新API缺失从未当编译失败证据。

- P1三个32KiB U+0001的part，各符合旧上限，literal98304而完整实际JSON590957字节仍被旧门接受，说明每part门不保证完整请求硬cap。原NUL夹具被旧text门合法拒绝，初始1/1日志及源码完整保留，不算产品缺陷；后来只改U+0001重跑。台账一处590948数字笔误已即时更正，原日志未改。
- P2旧编辑器allowed只是允许子集，能省略只反例/未选源。新fixture的required完整闭包另验，不悄改旧公共语义。
- Q3手算六种Abs/Deduplicate/Sort排列在非对称输入的实际输出，经完整编码、真实编辑编译、读取候选正文执行，再将候选JSON往返作新父二次生成no_change；六输入摘要不同、实际两案例不为空、所有包括count-only依赖保留，原Skill.dependencies不改；ProgramFixture与未验证历史轴分别保持。
- Q4在完整编码字节中注入unicode转义同名重复key（limits、method.program、parent），重新计算正确字节数与hash，仍固定fixture_invalid_request_json拒绝且不回显SECRET；原始合法请求的两案例和非ASCII父字段不变。

head探针运行前增强了cases.len==2/首轮正文实际改变，防止空for/all空过；旧版本源码分别留before-nonempty-assertions，实际父共同反例不受影响。

##### 实施证据和边界裁决

实施最终报告 `out/audit-20260930/qa/impl-ag076/REPORT.md` SHA `eba495ba8f3f7b8bcf5a8e27e55eb344f4daf81a619950e38491332595d8defc` 已全文读；完整证据逐项实际hash/大小核对并归档，`ag076-reviewed-delivery.json`记录索引。基线先正常Cargo no-run0；资源不足原run98未执行；R1只准原已编译纯内存二进制实际2/0，明确不是Cargo实际run成功。新能力no-run0后34/0。

两变异正常编译后：固定oracle输出变异29/5、绕过实际输出预留变异33/1。第一次执行变异在测试进程尚未收尾时恢复源码，实施方如实披露并完整重跑严格顺序no-run0→29/5→session关闭→restore/cmp0，原首轮日志保留；最终三源码逐字相同。恢复后四bin58/0、clippy/build0。原full1302/5为五HTTP本地bind EPERM，R2同完整命令1307/0；CLI首次readiness1仅该事实，不冒称stderr EPERM，同命令/实际binary环境复跑0。整理计数时默认Python脚本exit1后改用既定runtime成功，原诊断留索引，不是测试失败，也未因此重跑测试。

卡 SHA `da488e2bf452dc52035be6ab697c1bca994906628a0c77862872bed08bad2e9f`；R1资源 `1d04d0ef83b9e354196d300398714c4b3ba8b36283c4f27428b60c58a1ad7c05`；R2监听环境 `957fece09cb7f0752a164899430452882a85329daf2331d4af54934d27b4300a`。无产品返修，两环境/资源裁决；原失败保留不筛。纯cache回收不删除源码/QA。

##### 有限结论

本地verified仅E16.1/E02/E03可执行内存ProgramFixture：§5.3.1L495/499/503/507、§6.3L644/646/648/650、§6.4L658/664/684、§11.2L1100/1104、E16.1L1419–1423；V059/V089的完整编码与编辑边界、V076的内容因果前置切片。

可信调用方冻结control/P/B/全部202内材料引用；实际完整输入UTF8 JSON每字节一个本地fixture token，输入<=131072、输出<=65536、context<=196608，32案例/每例32整数/最多4封闭操作。实际同一字节序列解码并生成完整SkillEditBatch，再原解析/编译、保护区/4096限制。独立oracle不调用候选执行器；条件不满足Unsupported，0步identity/no_change不算学会。这里的token是固定字节词表，绝非真实模型估计；无真实provider调用、无伪造账单/应用/正式回执。

仍未接三reader、AG070材料读取、AG077预算、存储授权/撤销当前水位、持久Pattern/Candidate、正式门或新真实run；**完整V076/导入学习链仍未完成**，不能把本内核当整链验收。未知自然语言、真实tokenizer/模型/沙箱、正式批准、统计收益和发行不在可声明范围。

无PR、未push/merge、无admin，不计已合并产品/support-scope。AG071精确公共推送的自动审批拒绝与待人类授权仍挂起；本任务遵照主控未申请外发，没有自身外发拒绝。main变化须重新叠放并fresh独立验收。回滚参考父212c，既有账单/撤销/历史不逆转。

#### 2026-10-01T21:16:39.587933+00:00 启动闸门与 GA26 侦察归档
- 三hash匹配；auth/fetch0、cargo1.98.1/Python3.12.14，main/origin212c不变，df23120000KiB。最新状态追加controller-state。
- SC-BUDGET-INSPECT报告全文读取，保真SHA c67d011989a16b8dbae2c306401f27681174bbef6a768dabe15834db80ebd5a6；auxcursor61完成，无实施/构建。主控复核冻结migration0005和readonly恢复连接、真源L392/396/705/709/761/1077/1112/1134/1453。报告需独立显示已记账root和调用未决，不从非空actual推断settled，不把原预留当未知实际费用上界。078未派。

#### 2026-10-01T21:19:28.955509+00:00 AG078草案只读复核与077定向进展
- AG078-draft SHA7835e0bb3ec1ee34ea2437f1c04d17af48b8e92597a2697b6f3691c1627849dd，E04/E16.5 §3.5L392/§3.6L396/L705/709/761/1077/1112/1134/1453，V008/V096.c有限元数据诊断。child054 gpt-6.1-sol/max仅main只读审卡<=100行，无实施/构建；078暂无WT/正式派发。单页明示边界，root原账与page统计区分，不新授权/计费规则。
- AG077 child自报首轮no-run0、binding21项18/3，三个新夹具被旧对象id CHECK/4MiB拒绝未触目标门；保留首错，仅修新夹具后复跑。不是产品红灯，尚未root验收；其余18项actualpass。R1清理/恢复/真实三writer及变异仍在做，full尚未许。

#### 2026-10-01T21:24:44.009641+00:00 AG077实施中静态预审 R2
- 完整预读当前typed_budget451行与lib diff；发现private walk只计算artifact redacted，新模式因此不能按卡5拒绝仅run正文脱敏而无tombstone。裁决cards/AG-077-R2.md SHA 48f09175e7614652d92da1947802ea0bbe3abd78dafcd52e3cff2ae92a1b51da，依据§11L1077/1110/1132，private取真实标志、旧wrapper保留历史语义。要求同当前实现先可编译实际失败后修，未声称已有动态复现/验收。白名单不扩，child065最高档不变。
- root新API复合probe草稿static校正CleanupStatus无PartialEq及撤销时间单调后，head-new-api SHA87fb421a685775b7975efa6ef2445bd24a9aa5b0c9cfc7a226d6b3ca51272963、combined63b2e6e7648650aebc35d4882e30f3d50ba9cabb9cdf611dca430d06136c7c65。原未运行稿保留before-static-type-review；尚未在active WT编译。Q3迟到间接边/缺索引/逐步重启收敛，Q4同根双namespace同sourceID与同innerinput隔离。旧共同probe与父2/0证据不变。

#### 2026-10-01T21:27:55.849942+00:00 AG078正式派发
- E04/E16.5，V008/V096.c只读账单元数据页，卡cards/AG-078.md SHA c62a12f768721a042f6d272b519af78f5547b0c15dbb05e0b29e6b320c0a370f；SC-AG078-DRAFT保真记录SHA a4fb195b93b6b2b55aea7c1c3f6d6d483f2d064f37881c0ebd373c34f40a9289（完整原报告已读，保留所有建议及证据），原草案7835e0bb保留。主控把历史reserved正数、actual非负、dispatch实际ID计数、临时源复制位置、严格identifier/坏关系不被过滤、真实fixture角色逐项纳正式卡。
- child057实际gpt-6.1-sol/max（计费最高档）；当场main/origin212c78488b403397eaebaa65a6d280b9c6be3899。WT out/work-20260930/ag-078、分支wrokbot/ag-078-budget-report、独立target-ag-078、scratch ag-078-impl。两新Python文件与077文件不重叠，当前两实施。仅本地提交交付，不外发。
- df23080000KiB约22.011GiB，首阶段静态及Python可行；Cargo旧API真实样本先冻结源并申请短窗口，不抢077当前单路定向/返修。full由root统筹、不得过滤。

#### 2026-10-01T21:30:43.295681+00:00 AG079正式派发
- E03/§6.4.1L670–684/§6.4L662/§11.5L1132/E03L1225–1229，V087.a/b仅pure诊断被复制假设结构门。复用已完整SC-E03报告事实，root又实读core90–214/696–709和真源原文，明确早返回复制未经validate的hypothesis；生产可信应用归因未接线仍缺，不伪称全V087。
- 卡AG-079.md SHA ecc7fceceee4f461f259a74fadac750b523a766977b711773204106aa288064e；child054实际gpt-6.1-sol/max（拒绝边界最高档）。当场main/origin212c78488b403397eaebaa65a6d280b9c6be3899；WT ag-079、branchwrokbot/ag-079-diagnosis-hypothesis-structure、target-ag-079、scratchag-079-impl。只coreoptimization及新测试，与077/078不重叠，当前三实施上限。
- df23080000KiB约22.011GiB；初步仅静态旧API测试与golden准备，Cargo申请短窗口，077R2优先。原有效输入/None分类字节保持，旧前置错误保持；非法假设以原错误拒绝不丢字段、同ID同digest不额外拒绝。无新类型/阈值/旧测试修改。本地commit交付无外发。

#### 2026-10-01T21:32:33.432766+00:00 主控反例静态准备
- 077新API probe再次按实读旧require_call_access核定异namespace Host实际Err为NotFound非Forbidden；首跑前改断言，原稿留before-call-access-error-review。最终head-new-api6718d5cd5b7246a92ac760373164898634ab391d9f1c54723a33e6f3421e6d08、combinedf7229b13ac6387e63c0ad1068167fb381ac667caacd2bd49167477323c08b12b，尚未运行。
- 079 root共同旧API探针冻结：P1竞争无效unused receipt仍保持NotSelected前置语义，但serde往返的同ID支持/反例矛盾不得泄出；P2缺receipt/截断与非法行为digest/反例ID复合，None与有效字节保持、不回显秘密。源复制main原optimization test，未在任何WT运行；后续正常编译父红/head绿。

#### 2026-10-01T21:40:30.132942+00:00 续跑闸门、AG079 引用更正及构建协调
- 提示词b0000207、真源70ec06e4、v4.1归档45f3ba06匹配；auth/fetch0，cargo1.98.1、Python3.12.14，df22600000KiB。持续目标active，用户未收尾。
- AG079-R1.md SHA a0276fd555e881a4f564171d0dbc31e5e24b928071ed5a4e866aa6d0c0ab1ede：原卡ecc7f…及派发的§6.4 L662更正为L664 insufficient_or_unsafe，L662实际capability_request；原卡保留，白名单/实现/验收要求不变。
- AG077实施方报告R2基线no-run0后25项22/3 exit101，修复后25/0，session83745已关闭；root尚未独立实跑或读完最终证据，不当验收通过。旧Session语义正对照保持。
- AG078准冻结旧API生成器59ce03b4c5ef2254570ff9101a999e832668877140d7d7a7846d2e2c876d61a3短窗口no-run及实际执行，jobs2/df>=20GiB；077定向、root079父core反例合计最多三路，无full授权。079实施仍静态。

#### 2026-10-01T21:42:24.518999+00:00 AG079 主控实际父双反例完成
- 实际父212c784独立target正常编译后0/2 exit101，session12495实际关闭；日志qa/ctrl-ag079-adversarial-base-212c784.log SHA 50bbdc2d6bf1b26446862eb07344cfdf40d9e2cf825376875d3d36158f484f6f。P1未选择仍复制支持/反例同ID冲突；P2缺receipt与truncated仍复制非法行为摘要/反例ID。有效同摘要冗余及None对照通过；不是编译失败红灯。
- 源4e06d0cdafcfe829a89dfb2b1c48fb04f57b4ad382a262f0db71f67f02f9aef1、append6c5f252370cf791fa57f58e32d8665db4184885dd7708a5af48c6ba1fedf7981不变，事后WT clean；只回收停止纯target-ctrl-base-ag079，所有QA/WT保留。
- 077已完整静态预读当前5已跟踪文件diff与451行typed_budget；仍在实施，非最终验收。实施方报告restore5/0；writer首轮1/1为新fixture usable/blocked相同触旧校验，仅改新夹具再跑。另有传播orphan-ref边界在测试核实，暂不允许仅prefix扩大旧artifact分类。

#### 2026-10-01T21:43:40.847107+00:00 GC22授权合同只读核查
- 辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/max，卡SC-GC22-r4.md SHA 3ebdb052da9be6ddf0d661a611f52b419086605205e7a19ecd568d768612697f；仅当前main实读非零ExperimentPlan冻结/现有权威绑定/生产调用，不构建或实施，不凭自称digest扩大预算授权。三实施保持077/078/079。

#### 2026-10-01T21:44:27.756058+00:00 AG077 R3孤儿前缀探索裁决
- 卡AG-077-R3.md SHA f52288f932299ba2ffd649add20e3c87251f5247fa31b8eea4d0f3997449b55d；主控实读newcleanup试验及产品callback/扫描，两个无call/无费用人工节点的Failed预期不成立，原no-run0与8/2日志保留，不计产品红灯。只许新测试改为真实边界；不凭ID前缀扩大旧schema分类，合法typed newref无call仍须持久Failed。真实newcall三正文/缺ref严格要求不减。
- AG079获本独立target/jobs2短窗口no-run→真实旧golden→完整基线，当前077/078/079最多三路，每次df>=20GiB，full未许。

#### 2026-10-01T21:46:26.414537+00:00 AG072六小时周期预备卡
- 卡AG-072.md SHA fdef1d93b3f584e15cf4136e5a8f765000ae147ff8badf8e52cbe472f1aa90ff，主控E00/E16.6台账自做，三文件预期白名单，明确只新增已合并066/068两子范围。21:54:38Z触发时核最新main；当前仅准备卡/探针，不宣称已执行或已发布。无未合并产品提交进入本PR，公开台账授权来自启动§5/7。
- AG079实施方实际no-run0、真实父golden98条/66540字节SHA c322250c1ccf9587186791beb3c477ef0736e677df91799a8539b4dad3102801，完整基线8/9正常exit101、864非法cells两次返回Ok；productdiff0后才准三行门及定向/变异，root仍须独立验收。

#### 2026-10-01T21:46:54.277663+00:00 AG078真实旧API样本完成及停止cache回收
- 实施报告冻结59ce03…生成器no-run0、实际1/0，7scope/13call经公共API、无provider；临时Rust文件已移除，WT clean/productdiff0后继续Python。样本DBd783d371…、事实bb2ff012…待root完整审阅。mode=ro实际产生空WAL与32KiB SHM的事实已记录，不宣称无旁文件。
- 实施确认全部session结束且停止访问，主控仅回收target-ag-078纯cache约382224KiB，源码/样本/QA不删。AG077继续定向/旧回归，079定向/变异，均无full授权。

#### 2026-10-01T21:52:51.946922+00:00 AG079完整预审与构建协调
- root完整读3行产品diff及647行新测试，source43c1b81f…/test05ba94fe…与实施冻结相同；98条golden来自实际父，864非法cells矩阵/None/重复引用/前置错误保留，无生产可信归因声明。077新restore测试548行完整预读，哈希存qa/ag077-079-prereview-2153.json，尚非最终验收。
- 079实施自测17/0、旧core22组256/0，最初误报optimization18/0已纠正为实际4/0（skill_edit14/0、总256不变），两变异正常编译后实际红/恢复cmp0。准079先行完整fmt/clippy/workspace，build前不足20GiB须报；077结束变异及恢复定向后暂不full。
- AG072独立登记/五篡改probe已准备SHA4ce49e8cc4212b6663063411da4c75251cbc6e7fb46880c6540c8d501dab8650，尚未运行。

#### 2026-10-01T22:01:14.398727+00:00 续跑闸门、构建空间与返修裁决
- 提示词b0000207、真源70ec06e4、归档45f3ba06重核一致；auth/fetch0，cargo1.98.1/Python3.12.14；main/origin212c78488b403397eaebaa65a6d280b9c6be3899 clean。按序重读门禁文档；用户未收尾，继续。
- AG079原full1285/5、97组92bin5doc，五HTTP bind EPERM原段已读，session13105正式101结束。R2卡SHA 3e407a4fc1a96996daae74ec45f96c49ee0c81967bbbbc88331b40031c990e68，仅同完整命令监听环境重跑，无筛选；磁盘当前19840000KiB，先回收停止cache。
- AG077 R4卡SHA a7d899eb16ba746766817826b1ecf9a71dd6d5c2ae88340294251117dc2b99dd，private入口区分run JSON解码，旧wrapper错误语义保持；先实际复现再修，原prefull冻结保留。全部Cargo暂停等空间协调，079原已结束。

#### 2026-10-01T22:01:45.073360+00:00 停止cache回收
- 077全部session先前关闭、079另明确14条均关闭且无queued使用。只回收target-ag-077及079原日志92个已停止测试exe，路径/大小/hash保存在qa/ag077-079-stopped-cache-reclaimed.json。root独立求和079原完整1285/5共97组；QA、源码、WT、079既有依赖cache均保留。

#### 2026-10-01T22:02:50.078294+00:00 AG072 六小时台账正式执行
- #112真实合并15:54:38Z，六小时阈值21:54:38Z已到；此后#111/#113两产品已合并。续跑门禁及磁盘协调后开始，非第三产品触发。卡SHA fdef1d93b3f584e15cf4136e5a8f765000ae147ff8badf8e52cbe472f1aa90ff；主控自做E00/E16.6/V001/V071/V072/V073/V074/V080/V098有限登记。
- 当场基线origin/main212c78488b403397eaebaa65a6d280b9c6be3899，计划WT ag-072/branch wrokbot/ag-072-controller-ledger-r4d。先父缺登记/历史控制探针，后准备三文件；没有任何local产品提交或原始QA进入PR。079唯一构建中，072先静态和Python，fresh八门待窗口。
- 准备脚本运行前将#112临时占位替换为真实验收head a18cedd50ce5e65bd090f0e17d7a261c3e0e1698，并硬断言git第二父相等；脚本SHA c150f486e9cbed20c9fec1b061c7eb18b05605ccefaf1fa830ace170431c4006。此脚本此前未运行，未产生错误公开台账。
- SC-GC22完整原报告保真存cards/SC-GC22-r4-report.md SHA 2c7b8505ac8f8f71bbad5ea15c6f951bc593d08520beab50d24ecee1e9f9f429；只读结论非动态验收。已有实际授权消费者，freeze缺可信授权接口合同；QueryBook::open现有计划验证缺口另待真源/卡/实际基线，不将自报摘要放行。


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

### 第四轮本批事件逐项折入

#### 2026-10-01T14:07:21.271228+00:00 AG062交付发布审批冻结
- 子代理本地5d70466ce8b90164ddc14bd120fb20a8fcd3cb22，clean，2文件+460/-0，自测1208/0（1195+13）全部门0；尚未root验收。push被auto-review拒绝，理由未识别可信人类对具体repo/payload授权，已冻结，无PR。原文out/scratch/ag-062-impl/external-rejection.log。root将独立读完整diff/真源/精确remote再据本chat直接指令申请限定复核，未获准不发布、不绕过。

#### 2026-10-01T14:11:30.840941+00:00 AG062完整独立payload审阅
- root已完整读2文件diff、454行新测试、最终report、原拒绝和payload/PR正文；固定5d70466ce8b90164ddc14bd120fb20a8fcd3cb22，+460/-0，只有门6行，R1及旧校验顺序/合法输出边界符合。全部顶层scratch归qa/impl-ag062。
- 直接用户哈希文件再次匹配b0000207…，origin=https://github.com/acosmi/RSIAgent.git，核§3.4明文指定draft目的地与§10E02后续队列；不把代理消息当授权。原拒绝理由及新核事实提交同一限定push的auto-review复核，尚待结果；无替代通道。

#### 2026-10-01T14:12:11.888716+00:00 AG063/#107交付与独立全量
- draft https://github.com/acosmi/RSIAgent/pull/107 已创建并附chat，head1bf70d3cc51e9a5ecf3eb1314de2221abf32042c，父279ee7c5290fabbb20cbd16b7e53c5ac2b71c188。3文件+285/-15，全文新摘要/每个索引/合并表与当前状态行逐项核对；逐事件fold仅标题升层机械相等，SHA256ef9be7309881bdf01643b1fd966e2cc5589c75123d7ab6791068a1506d3d6a2a；所有旧verified记录相等。
- 基线registration真实exit1缺四记录，precommit checker structure_valid=true/errors=[]。首次sandboxstage因index.lock权限128后获准本地提交/推送，非审批拒绝。全新target-ctrl-ag063八门运行，预期1202/0，不往运行中WT添文件。

#### 2026-10-01T14:14:11.498607+00:00 SC-GA23完成、权限合同范围裁决
- /root/scout_ga23完整只读回报已读；落点摘要cards/SC-GA23-r4-report.md SHA256b22040ba2ad1a3b8d1946aa00223293ea1fe1d93af84c49e7333b0744ef2107c。现有shape/dev/ino/nlink不是mode/owner策略；真源E16.5L1451未定义Unixmode/UID/ACL/组共享，默认data可能使用cwd，不能擅制0700。将该具体政策挂起；路径身份/访问失败前置可另行真源裁决，尚未派实施。
- AG062直接用户授权/精确payload复核的同一限定push已获auto-review允许并成功，draft创建中；原拒绝仍保留。

#### 2026-10-01T14:21:27.622769+00:00 压缩续跑闸门与SC-GB16归档
- 提示词b0000207…、真源70ec06e4…哈希匹配并完整重读启动规则及规定材料。gh auth首检exit1报token invalid，只读重试exit0，无改凭据；fetch0、cargo1.98.1/Python3.12.14、磁盘31GiB。main=origin/main=279ee7c5290fabbb20cbd16b7e53c5ac2b71c188，clean。
- AG062 draft#108已创建并附chat，固定5d70466ce8b90164ddc14bd120fb20a8fcd3cb22；待#107后叠放与主控全量1215。AG063/#107全量仍在收尾，不写WT。
- 辅助窗口SC-GB16完整报告归档cards/SC-GB16-r4-report.md SHA25694e10b77436d452be07ccd045a2cdeddb2479572815e9a311e7802bef89c6282，root完整读回；episode计数/失败谱系与旧report重算失配边界须裁决后实施，尚未派卡。

#### 2026-10-01T14:22:27.745927+00:00 派发SC-GB16 R2
- 用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/high（M/L只读），基线279ee7c5290fabbb20cbd16b7e53c5ac2b71c188；卡SC-GB16-R2-r4.md SHA256bfaa6034862d0e8ee1efa3d31e500699bf0a9542e7f249294591e20e0108b467。核旧report重算/重复ID/下游消费者/最小旧测试授权，不写入不构建；无实施分支。

#### 2026-10-01T14:23:52.454592+00:00 CTRL-AG063-R1 / #107独立验收通过
- 固定head1bf70d3cc51e9a5ecf3eb1314de2221abf32042c，父279ee7c5290fabbb20cbd16b7e53c5ac2b71c188，无叠放。3白名单文件+285/-15全部复核；原事件折入仅标题升层，fold SHA256ef9be7309881bdf01643b1fd966e2cc5589c75123d7ab6791068a1506d3d6a2a；所有旧verified记录相等。
- 全新target-ctrl-ag063，qa/ctrl-ag063-1bf70d3.log*八项exit0；1202/0=父1202+0，85二进制+5doc组，Python48；定向ctrl-ag063-support-scope-1bf70d3.log14/0。
- 主控probe-ag063/probe.py SHA2564f762d85fa4fa3f3d40554db343b750b5fff8a36b3d664139bc233b0ebc19d18；head ctrl-ag063-adversarial-head-1bf70d3.log全部通过，实际父ctrl-ag063-adversarial-base-279ee7c.log缺四记录exit1，历史篡改/正向/clean通过。四项source/merge两父/tree/祖先/testblob/input digest/日志hash与真实计数逐一核实。T0旧AG042输入、T1新AG055输入、T2跨scope日志替换、T3错merge、T4新AG057输入均拒绝；历史篡改与结构拒绝为已有边界，父登记失败是覆盖缺口，非产品缺陷。两树同方法复制本地受测input并finally删除，tracked clean。
- verified仅E00/E16.6，E00L1181–1189/E16.6L1459–1463/§18.5L2162–2166，V001/V071/V072/V073/V074/V080/V098四项已验子范围登记与可追溯。AG055/054/058/057各自原验证范围和失败证据保留，不扩为完整E/V。E01仍blocked、GA14阶段计划绑定/GA23文件系统政策缺合同、E03诊断真实证据缺口和056workflow权限阻塞明确。
- 不可声明新产品、历史包归并、完整默认范围、真实效果或发行；旧历史部分input缺失保留。回滚参考父279ee7c并保留证据/费用/撤销事实。无返修、无冲突；按启动§5索引任务采用Python反例而非Rustappend。首次本地index.lock权限失败获准后正常提交，未有auto-review拒绝。

- 2026-10-01T14:24:13.716467+00:00 主控合并脚本生成器首轮把固定SHA出现次数误断言为7（实际5）而在写脚本/远端操作前退出；纠正该本地断言后生成同一已审脚本，未执行任何错误合并命令。验收数据与head不变。

#### 2026-10-01T14:25:50.901835+00:00 合并AG063 / #107
- fixed head1bf70d3cc51e9a5ecf3eb1314de2221abf32042c，merged_sha3f9ed595bdcd8bf241bc73fb83640d277947810a，mergedAt2026-10-01T14:24:58Z；双方treec3a5417f233ff083b90eee137338ee8303ad5d1a相同，实际父279ee7c5290fabbb20cbd16b7e53c5ac2b71c188+验收head。fail-fast全部前置通过，GraphQL EOF/fetch TLS原请求重试成功才继续；admin=false，main已ff-only。返修0，无冲突，台账周期归0。二次清单即刻更新，清理随后。

#### 2026-10-01T14:27:24.931131+00:00 SC-GA10只读派发
- /root/scout_ga10，gpt-6.1-sol/high（M侦察），基线3f9ed595bdcd8bf241bc73fb83640d277947810a；cardSC-GA10-r4.md SHA256f88414332f664d512fc29c73be50e5c045d3fd535a3ee36205cf4e41564b2839。E02/E16.4、§5.5/V060–V062实际HostSurface差异/消费者/最小范围，不实施、不构建；与辅助SC-GB16 R2共2只读。

#### 2026-10-01T14:28:15.652012+00:00 AG062/#108叠放
- 单提交5d70466ce8b90164ddc14bd120fb20a8fcd3cb22从9b08c4b2fd882abab65f48ea0af8f23b13a3b50d叠至3f9ed595bdcd8bf241bc73fb83640d277947810a→e6e7eec47a3494425a1842604c154d8695089222；前后patch逐字节cmp0，无冲突。首次固定fullref/oldhead lease推送TLS失败，脚本立即退出，完全相同调用重试。
- #107已合并台账/二次清单/命令AW即时记录；两工作树正常remove、branch-d/remote delete完成，target清理中，QA保留。

#### 2026-10-01T14:29:17.093339+00:00 AG062主控全量和父树探针启动
- literal lease重试push成功，新head e6e7eec47a3494425a1842604c154d8695089222，fresh target-ctrl-ag062运行ctrl-ag062-e6e7eec.log*，预期1215/0=1202+13，期间不向其WT添加文件。
- 实际父base-3f9ed59/独立target-ctrl-base-ag062运行两root反例，冻结source/append不变，仅旧API；#107清理全部完成，磁盘34GiB。

#### 2026-10-01T14:32:51.894447+00:00 AG064派发前设计
- 主控已独立核core批次仅按branch去重、engine目标node计数和旧AG042差异断言；cards/AG-064-design-r4.md SHA256e8acb74481f71dce2f96a09fa6c694fd154348a9a35f7faf41563e82a582d901记录最小跨crate共享资格/episode同批去重方案，E09/E10/V024/V081/V086子范围，仍待只读R2完成核cache事实后正式卡。无实施授权。

#### 2026-10-01T14:33:34.766924+00:00 SC-GA10完成
- 完整/root/scout_ga10报告已读，主控存档摘要cards/SC-GA10-r4-report.md SHA256997badcd86959990335a8c5ffce7c2f5175aef0f913551b9fe2e9b2f9957ac10。§5.5L527明确旧项不存在拒绝，contract.rs仅单向覆盖，删除/非空截断漏检可用原API最小修复；与AG062同文件，等待062验收再派独立AG065。完整类型/默认值/真实提取/Tool-only映射合同缺口保持，不冒称全部V060–62。

#### 2026-10-01T14:36:21.240749+00:00 CTRL-AG062-R1 / #108独立验收通过
- 交付5d70466ce8b90164ddc14bd120fb20a8fcd3cb22，原基线9b08c4b2fd882abab65f48ea0af8f23b13a3b50d；叠至3f9ed595bdcd8bf241bc73fb83640d277947810a→固定验收e6e7eec47a3494425a1842604c154d8695089222。唯一提交前后patch逐字节cmp0，无冲突；literal fullref/oldhead lease首次TLS失败原请求重试成功。2白名单文件+460/-0全部已读，产品只compile_improver入口6行，新测试454行13项。
- 全新target-ctrl-ag062，qa/ctrl-ag062-e6e7eec.log*八项exit0；1215/0=1202+13，86二进制+5doc组；Python48。self旧基线1208/0；81个缺P/B组合实际接受导致baseline10/3，去P/去B变异各12/1，恢复13/0。四完整产物JSON/所有fingerprint逐字相同。原clippy循环告警及日志parser漏1标记的初错、修正、auto-review拒绝均保存在qa/impl-ag062。
- root probe-ag062/source-core-golden-c0ea35d.rs SHA256b24ae22a8dfcf2c8aa8ee7b60ea525d8f00fec99516b04c85ca3032a59b7780a；append.rs SHA2561f4abf5cd3476056bf115b013874b3db4f9c71d3850719e9c3570cdb8e85ce54。head ctrl-ag062-adversarial-head-e6e7eec.log2/0 exit0，实际父ctrl-ag062-adversarial-base-3f9ed59.log0/2 exit101且正常编译；双方探针后clean。P1真实JSON往返、空id、evolution真假、全Set及重复调用缺P/B仍拒，非空NUL/tab标签继续旧允许；P2缺上下文先于无效Set输出值拒绝，合法标签时旧值错误不变。父P1错误接受，P2返回补丁值错误而未核上下文，非编译失败。
- verified仅E02，§5.3L473/L487–491、§5.1L453/L455、E02L1211/L1215；V046缺P/B基础拒绝与V043三态合法兼容子范围。保留parent/baseline Strategy原错误优先，所有三态含全Set均需两个非空标签。R1纠正原卡空instruction措辞，现有Strategy空/白/NUL等拒绝不变。
- 不可声明真实P/B内容摘要/批准/存在/撤销/profile绑定、非空标签真实性、完整FieldContract或未实现max执行消费者、全部E02/V046、真实宿主/效果/发行。旧v1结构/字节/算法不变。回滚仅移除门和新测试，参考父3f9ed59且保留历史/费用事实。
- 产品返修0，R1测试口径裁决1；无冲突。子代理push自动审批拒绝后冻结；root完整payload/remote/直接人类指定哈希文件§3.4/§10复核，限定同请求新证据审批获准后发布，原拒绝保留，未绕过。主控全部独立验证通过后才ready/固定head合并。

#### 2026-10-01T14:37:09.961253+00:00 SC-GB16 R2完成与AG064范围裁决
- 完整报告cards/SC-GB16-R2-r4-report.md SHA25689367b40bec50b9fb817a2841b84610f56bcda726767734ced8e9a000264e164，root已读；更正R1简述：report ID已含固定engine版本但无代码SHA。存储静态load可读旧报告，engine verified/重复pool run重算不一致拒绝；不会刷新同ID，已有job元数据不等于评分重验。
- 根据§7.1L787/§7.1.1L793,L805,L809/§7.2.1L825–831/V086，正式AG064含core共享谱系门和同批episode去重，engine只派生已观察Recover事实并投影。保持schema/engine常量，旧不兼容评分在现有语义消费门失败关闭，不覆盖/迁移；真实模板/reset/lease和正常Deepen/PolicyStop分类等明确不属本次。

#### 2026-10-01T14:38:06.190130+00:00 合并AG062 / #108
- fixed heade6e7eec47a3494425a1842604c154d8695089222，merged_sha6f0e5fa35b582f0f4cde4207ddf59d65a51b180a，mergedAt2026-10-01T14:36:56Z；双方tree2612aa2fe849986b20eeffe43a4ff759bde6eae7相同，实际父3f9ed595bdcd8bf241bc73fb83640d277947810a+验收head。fail-fast全部前置成功，fetch TLS原请求重试成功才继续；admin=false，main已ff-only，返修0/R1测试裁决1，无冲突。#107后产品1项，二次清单和输入即时更新，清理随后。

#### 2026-10-01T14:40:14.889656+00:00 派发AG064
- E09/E10/V024/V081/V086仅恢复派生/谱系与同批上限；正式卡AG-064.md SHA2569cadb4054a444c15e83ae90530bc836b4ebb9ea6c97831987a174707d06301a5，取代设计草案。基线6f0e5fa35b582f0f4cde4207ddf59d65a51b180a，WT ag-064，branch wrokbot/ag-064-replay-episode-recovery；用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/max（L跨crate/恢复安全）。6文件白名单，基线旧报告实跑和兼容拒绝必验，schema/engine常量不变。实施1，编译许可已给。
- #108清理全部完成：两WT clean/祖先核验后正常remove，branch-d/remote单独delete与3targets清除，QA保留，磁盘37GiB。

#### 2026-10-01T14:42:25.178437+00:00 派发AG065
- E02/E16.4/V060名称漂移/V061非空截断子范围，正式卡AG-065.md SHA2562de54bf3ae1a50237c9a15cfa8551cd0c75f53c56dde1127008bc97a980f04d8。真源§5.5L523–529，最小反向存在门在原校验后，三个classification同等，保持所有旧schema/digest/错误优先/duplicate完整集合语义。
- 基线6f0e5fa35b582f0f4cde4207ddf59d65a51b180a，WT ag-065，branch wrokbot/ag-065-host-surface-missing-names；/root/impl_ag065，gpt-6.1-sol/max（M安全拒绝/持久读取）。与AG064六文件无重叠，AG062同文件已合并后才派。实施2/构建最多2，root未来验收最多3路。

#### 2026-10-01T14:43:33.304555+00:00 SC-GA21只读派发
- /root/scout_ga21，gpt-6.1-sol/high（M），基线6f0e5fa35b582f0f4cde4207ddf59d65a51b180a，卡SC-GA21-r4.md SHA2565dd5ae5bf422d1b8f85904ff833f1637e75249074489dd6db8b950b4269db6d2。E16.4 MCP stdio真实入口/本地一致性harness事实，不写不构建；不等于真实宿主支持，不派CI。实施仍2。

#### 2026-10-01T14:52:55.757898+00:00 压缩续跑闸门与SC-GA21完成
- 提示词b0000207…与第一真源70ec06e4…匹配，完整重读提示词并按顺序重读规定材料（输出截断部分单独补读）；gh auth/fetch0、cargo1.98.1、Python3.12.14，磁盘40GiB。main=origin/main=6f0e5fa35b582f0f4cde4207ddf59d65a51b180a，clean。
- SC-GA21完整只读报告已读；本地真rsia子进程、现有2025-06-18握手、四golden、未知字段/可信身份隔离、持久重复请求与严格帧可独立测；取消传播/真实Claude/类型默认值提取/业务v2仍无本轮证据，绝不混称。AG064辅助chat进展：旧产品9回放测试4失败、旧report已实跑冻结，开始实现；AG065基线测试编译中，均尚未验收。

#### 2026-10-01T14:55:26.125798+00:00 派发AG066
- E02/E16.4、V002本地v1接口/V039平台宿主声明边界；卡AG-066.md SHA2560e995b9ad068fc5d99f2ffab51105babd1262b0bd2f3fd94458567f1d448219c。派发前核真源V039 L1539并把草稿错误的身份子范围措辞更正为平台/宿主声明边界，尚未实施时修卡。SC-GA21摘要03ede9cf567ddf7ffb6c7d2bda7c077b10241a206a41c078defae7f0d05a3908，原完整报告root已读。
- 基线6f0e5fa35b582f0f4cde4207ddf59d65a51b180a，WT ag-066，branchwrokbot/ag-066-mcp-stdio-harness；/root/impl_ag066 gpt-6.1-sol/max（M协议/安全测试），唯一新增scripts/test_mcp_stdio.py。真源§6.1L605–609/E16.4L1443–1445，本地fixed SDK3.3.0/disabled范围，真实Claude/取消传播/v2等缺口保留。与064/065文件不重叠。实施3、Cargo最多3，root暂不启动编译。下一台账号改AG067。

#### 2026-10-01T14:58:06.902756+00:00 SC-GA19只读侦察派发
- 新spawn因agent thread limit未创建，读取agent状态后复用已完成057代理/root/impl_ag057开展独立只读SC-GA19；模型gpt-6.1-sol/max（沿用原max，比M侦察high更高）。基线6f0e5fa，E16.1L1417–1423/§6.3/V076，核导入→优化→新run→撤销链实际API与ProgramFixture发布拒绝边界。禁止写入/构建/他人WT；不是实施、不占3路Cargo。
- root自写AG064两反例已冻结source SHA256c21745c6c21ef0dc8856923ad33a867f9b3a86f3348040dd9973bec368658c31，append103a95c6b019612abbd31c198d43c61da077d3e960d927df9abfb7f686b0201f：公平首选去重初始化/其他合法填槽，以及W4 Cancelled或UsageUncertain同批episode消费与费用。尚未编译，当前3路实施Cargo运行，root等待可用构建槽。

#### 2026-10-01T15:00:20.670137+00:00 后续边界入队
- queue-r3追加AG064未覆盖正常Deepen/终态合法集，以及旧报告静态读取/语义重验/无刷新协议；AG065名称门不关闭类型默认值/提取消费者缺口；AG066本地harness不关闭真实Claude/在途取消传播。依据和行号见queue，均为既有剩余范围显式化，不擅自发明产品协议或暂停整个流程。

#### 2026-10-01T15:03:11.294410+00:00 AG065-R1执行环境裁决
- 首workspace1244/5 exit101，5项均既有HTTP localhost bind PermissionDenied，非代码断言；原日志保留且不计通过。cards/AG-065-R1.md SHA256eb6a9dbbcbbf2441b39afbd69cc2808711f884fa10eb2d3156cff007f14d2ffa，依据E00L1185/启动§3.7授权向工具申请相同完整命令监听权限重跑；不改测试/产品/范围，不绕过自动审批。max不变，产品返修0，R1环境一次。

#### 2026-10-01T15:04:35.524708+00:00 AG064-R1执行环境裁决
- 辅助chat首workspace1224/5 exit101，同样5项既有HTTP localhost bind PermissionDenied，未改service.rs。AG-064-R1.md SHA2566bae9206616698fa1f680646ccb4a156b2a8bfbcc01f459fdf14e217d16a9fdb，只准工具审批重跑相同全量命令/独立target，不放松源码/白名单；原失败保留，产品返修0/max不变。

#### 2026-10-01T15:06:50.456421+00:00 AG065预审与父反例启动
- 实施报告R1后workspace1249/0、88bin+5doc及fmt/clippy/build/2smoke全0，仍待主控独立验收。root已完整预读产品10行门和两个新测试227+468行，3文件内容SHA冻结qa/ctrl-ag065-preread-hashes.json，最终交付需再次逐字核同。
- 先建base-6f0e5fa-ag065、独立target-ctrl-base-ag065，运行root冻结probe-ag065（legacy重复注册/跨namespace与evolution开关）两反例，日志ctrl-ag065-adversarial-base-6f0e5fa.log；实施已结束构建，root+064+066最多3路。最终head父必须仍为该SHA，否则重建实际父对照。尚未称通过/合并。

#### 2026-10-01T15:08:29.892755+00:00 AG065固定本地head独立全量启动
- 本地2787e0c19f4b73298e73c0ddead0f959ceb6bfef，父6f0e5fa35b582f0f4cde4207ddf59d65a51b180a，3白名单文件+705/-0；逐文件预审SHA与最终提交逐字一致，clean。PR创建待实施交付，外部head核对仍待补，不合并。
- 全新target-ctrl-ag065、ctrl-ag065-2787e0c.log*启动，预期1249/0。运行期间不向WT写文件。父probe已成功编译并0/2失败exit101，P1旧legacy相同ID重复注册仍Ok，P2evolution真假/跨namespace同ID坏配置仍Ok且留下snapshot；另正确scope正向/immutable冲突保持。baseclean；最终head后同探针验证。

#### 2026-10-01T15:09:34.853602+00:00 AG064交付payload与外发拒绝复核
- 固定本地d8641b28f9fdd354c50302625337edf10369b9b9，父6f0e5fa，clean；6白名单文件+1159/-48全部diff逐行独立读完，包括4+10新测试和3份旧报告完整JSON。原始材料顶层文件归qa/impl-ag064，含初capture编译/fixture错、directed初错、1224/5环境失败与1229/0最终全量；最终报告待补。
- 子chat限定push被auto-review以缺可信用户对目的地/载荷授权拒绝，原文external-review-rejection.json保留；未外发/无PR。root再次核本chat直接人类哈希指令，prompt哈希b0000207…一致，§3.4明确acosmi/RSIAgent产品draft流程、§10指定后续E09/E10队列，与§7本地材料限制一起核：该payload仅六授权产品/合成测试，不含out/方案/归档/凭据。origin实际https://github.com/acosmi/RSIAgent.git。以这些新核事实申请同一限定push审批，不用替代通道；尚未主控验收，不合并。

#### 2026-10-01T15:10:52.795614+00:00 AG065外发拒绝与可信事实复核
- 子代理push未执行，auto-review指缺直接人类对远端/载荷授权且remote归属/隐私未核，已冻结。root三个文件完整读与提交SHA逐字核已成，原QA归qa/impl-ag065，最终report稍后补。
- 本窗口直接用户指定b0000207…提示词逐条执行，§3.4指定acosmi/RSIAgent产品draft流程、§10 E02后续队列；payload仅3授权文件+705/-0，不含out/方案/archive/凭据。root只读gh API实核full_name=acosmi/RSIAgent、owner=acosmi、private=false、当前identity permissions admin/push=true；ag065 origin与该URL一致。将上述新可信核验作为同一限定push审批补充，不绕过原拒绝或使用他通道。
- AG064同一限定push复核获准，首TLS失败原命令重试中，无PR尚不合并。

#### 2026-10-01T15:12:48.968325+00:00 AG064/#109创建及AG065全量完成
- AG064限定push获准后TLS原命令重试成功，draft#109 https://github.com/acosmi/RSIAgent/pull/109 创建并附chat，fixed d8641b28…；仍待065合并后叠放/全量，不先ready。AG065同一push新可信核验审批获准后TLS原命令重试成功，draft正在创建。
- AG065 root fresh全量ctrl-ag065-2787e0c.log*八项exit0，1249/0=1215+34、88bin+5doc、Python48；同head反例启动。全过程运行时未改WT；父两反例已0/2实际缺陷失败。

#### 2026-10-01T15:20:26.013452+00:00 压缩续跑闸门与 AG066-R1
- 提示词与真源 SHA 完整匹配，规定材料按序重读，截断处补读；gh auth0、fetch首 TLS128重试0、cargo1.98.1/Python3.12.14，磁盘28GiB。未更改凭据。
- AG066首workspace1210/5 exit101，5个均旧HTTP localhost bind PermissionDenied；R1环境裁决 cards/AG-066-R1.md SHA256b56f9ddefe9863edf7e3f89cfe03a52b2fd45c70bccef3ef5241dc8009a49824，只准工具审批相同全量与smoke复跑，保留原日志，不放松产品或断言。max不变、产品返修0。

#### 2026-10-01T15:22:01.932615+00:00 CTRL-AG065-R1 / #110 独立验收通过
- 固定head2787e0c19f4b73298e73c0ddead0f959ceb6bfef，父6f0e5fa35b582f0f4cde4207ddf59d65a51b180a；无需叠放。3白名单文件+705/-0全diff已审、预审SHA与交付逐字相同；产品只10行反向名称门，新测试16+18。PR #110与WT一致且clean。
- 全新target-ctrl-ag065，qa/ctrl-ag065-2787e0c.log*八项exit0；1249/0=1215+34，88测试二进制+5doc组，Python48。实施最终报告qa/impl-ag065/AG-065-report.txt（09aa1f52acba01ea67d38a5047a5d875dea527c9a4fed72252df417a4229e55a）已完整读；原baseline15/19、移门15/19、最终34/0及沙箱1244/5均保留。
- root冻结probe-ag065/source-release-store-6f0e5fa.rs SHA256c01ea240e0646b6eeecb2e78db1a5707275b8080feee99a5bbac8c7f81119697；append.rs SHA256723dcb1d47833493f33730e8b25d67fa66f98a9268c7095f9c8cd158b549806d。head ctrl-ag065-adversarial-head-2787e0c.log2/0 exit0；实际父ctrl-ag065-adversarial-base-6f0e5fa.log0/2 exit101，均成功编译/事后clean。
- P1旧缺项record同ID JSON往返重复注册仍须拒；同ID改完整内容不可覆盖不可变记录，新ID完整换序正向成功。父错误Ok两次。P2跨namespace相同IDs、evolution真假各重复新prepare，坏scope均须精确拒MODEL且无run/artifact变化，好scope相同ID成功并幂等；父错误Ok四次且留下snapshot。root未声称这两个probe扫描audit；实施独立34项包含audit核验。
- verified仅E02/E16.4，§5.5L523–529/§5.6L541/E02L1215，V060名称删除或重命名导致旧名消失、V061非空截断子范围。全部旧校验后按manifest顺序首缺项，三个classification同等，精确case/Unicode，无去重/排序/schema/digest/错误优先级改变。
- 不可声明类型/枚举/默认值漂移、真实提取与完整性、消费者真实性、ToolOnly映射、真实Claude/全部E-V/效果/发行，已完成run冻结快照复用不在本门覆盖。回滚参考父6f0e5fa并保留既有记录/费用/证据。
- 产品返修0，R1环境裁决1（AG-065-R1.md eb6a9dbbcbbf2441b39afbd69cc2808711f884fa10eb2d3156cff007f14d2ffa），原卡2de54bf3ae1a50237c9a15cfa8551cd0c75f53c56dde1127008bc97a980f04d8；无冲突。子代理push自动审批拒绝后冻结，root核直接人类哈希指令/实际payload/远端public且当前admin-push权限新事实，同一限定请求获准后成功发布，原拒绝/首次TLS保留；报告写“未创建PR”是root接管前事实。合并前必须fail-fast正文逐字读回/fetch/head/parent/clean。

#### 2026-10-01T15:22:01.932615+00:00 SC-GA19完成与R2只读派发
- 完整报告已读：V076现有测试仅内存/手工Pattern编辑，实际持久导入生成blocked，TrustedRun/TrustedHost优化拒导入身份，ProgramFixture不能批准，新Host run无Active只能baseline；不以tests-only拼成正向全链。
- /root/impl_ag057继续SC-GA19-R2，沿用gpt-6.1-sol/max，只核rsia.optimization.source.v1作为派生非源run的unknown_scope清理分类、读取者/恢复/真源与最小修复白名单；不写不构建。当前仅066一路实施/编译。

#### 2026-10-01T15:23:19.693620+00:00 合并AG065 / #110
- fixed head2787e0c19f4b73298e73c0ddead0f959ceb6bfef，merged_shaeb275812f09dd8850ad4edac08a2282cb5e82112，mergedAt2026-10-01T15:22:30Z；accepted/merged tree=b95696ec74a88f2fa4fbe2ae42baccf36f973908，实际父6f0e5fa35b582f0f4cde4207ddf59d65a51b180a+验收head相等。全部fail-fast前置成功，admin=false（CLEAN无需覆盖保护），main已ff-only。产品返修0，R1环境1，无冲突。#107后产品2，清理随后，QA保留。

#### 2026-10-01T15:25:05.683982+00:00 AG065清理与SC-GB16-R3派发
- #110干净且祖先核验后正常remove AG065/父树，branch-d、remote逐个delete、3专属targets删除，QA保留，磁盘34GiB。
- 用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc继续只读SC-GB16-R3，gpt-6.1-sol/high，卡out/audit-20260930/cards/SC-GB16-R3-r4.md SHA2565c8063bddbfd13441996006f2dbaba2d4574f68ba914ba03ae0bc53b440b07f0；基线eb27581，正常Deepen/合法集与终态范围，不实施不构建。上一AG064完整report归档sha32b37883ec899630f8b6cd2c1075974b9fd4f76f7a2bcaddc8c37406f4ac1325；全部早期编译/夹具错已读，不作产品失败证据。

#### 2026-10-01T15:25:53.681064+00:00 AG064/#109叠放与主控全量启动
- 单提交d8641b28f9fdd354c50302625337edf10369b9b9从6f0e5fa叠到main eb275812f09dd8850ad4edac08a2282cb5e82112→437af3fc7e414a3509bbb3f9d2cc8a614f231616，逐提交patch cmp0、无冲突，literal完整oldhead/fullref lease成功。
- 全新target-ctrl-ag064跑ctrl-ag064-437af3f.log*，预期1263/0=1249+14、90bin+5doc。不得往运行中的WT加文件；root父probe用base-eb27581-ag064/target-ctrl-base-ag064，实施066加两root最多3路。

#### 2026-10-01T15:28:23.980977+00:00 AG064父反例实际复现
- ctrl-ag064-adversarial-base-eb27581.log 两测试成功编译后0/2 exit101。P1公平首选14后又选同episode11，W2/W4及反序/opaque改名全失败；P2 W4 Cancelled/UsageUncertain都选4个Recover而非2，8节点/8成本而非6，真实公开run_replay，世界未变。父树clean。head等full结束才运行相同probe，不修改运行中的WT。

#### 2026-10-01T15:32:11.406592+00:00 CTRL-AG064-R1 / #109独立验收通过
- 交付d8641b28f9fdd354c50302625337edf10369b9b9原基线6f0e5fa35b582f0f4cde4207ddf59d65a51b180a，单提交叠至eb275812f09dd8850ad4edac08a2282cb5e82112→固定437af3fc7e414a3509bbb3f9d2cc8a614f231616；before/after-rebase.patch逐字节cmp0，无冲突，literal完整oldhead/fullref lease成功。6白名单文件+1159/-48完整diff已审，只有2产品文件与2新/2授权旧测试。
- 全新target-ctrl-ag064，qa/ctrl-ag064-437af3f.log*八项exit0，1263/0=1249+14，90测试二进制+5doc组，Python48。实施完整report归qa/impl-ag064/report.md SHA25632b37883ec899630f8b6cd2c1075974b9fd4f76f7a2bcaddc8c37406f4ac1325；原capture借用编译错、缺baseline来源夹具错、Stored缺Debug编译错均已读/保留，不计产品失败；最终1229/0和R1前1224/5保留。
- root冻结probe-ag064/source-replay-v41-6f0e5fa.rs SHA256c21745c6c21ef0dc8856923ad33a867f9b3a86f3348040dd9973bec368658c31；append.rs SHA256103a95c6b019612abbd31c198d43c61da077d3e960d927df9abfb7f686b0201f。head ctrl-ag064-adversarial-head-437af3f.log2/0 exit0，实际父ctrl-ag064-adversarial-base-eb27581.log0/2 exit101，正常编译且两树clean。P1公平首选14的episode必须在填槽前占用，W2/W4、反序/opaque改名排除11仍填12/13/15并核总cost；父选择同episode11。P2真实W4回放两个alias对、Observed Cancelled/UsageUncertain，必须只选5/7、6节点6探测6成本且共享counter1、unknown为Censored；父选4次Recover而8节点/8成本。世界原字节双方不变。
- verified仅E09/E10，§7.1L785,L787/§7.1.1L791–809/§7.2.1L825–831/E10L1323–1329/§5.1L453/§7.4.1L853–855；V024、V081、V086.b/d限可信前缀失败谱系、同批episode上限、本次已观察Recover事实派生与全批投影。旧status counter不当事实；Observed取消/未知消耗一次，OOS/缺历史/显式Censored旧路径不伪造dispatch。
- 三份真实旧程序生成合成报告完整字节已审，affected SHA64b34091cdbd9b520257d526153257e164bd9db247f332ebfde0990fda5e1137 static load可读但engine重算和重复同ID拒绝、不覆盖；正常Valid/Hard旧报告保持verified/幂等。schema/engine版本/ID不变，新旧同ID不可用覆盖当迁移。已有job元数据不等于评分重验。
- 不可声明真实repair模板/reset/lease/费用生命周期、任意伪造prefix防护、跨根episode身份关联、正常Deepen已展开父/一般available_actions/PolicyStop分类、旧报告迁移、完整E09/E10/V或效果/发行。原AG042顶部旧按node计数注释在授权两函数之外，明确仍有过期文字；本卡未改。回滚参考实际父eb27581，保留世界/不可变报告/费用/撤销证据。
- 卡AG-064.md 9cadb4054a444c15e83ae90530bc836b4ebb9ea6c97831987a174707d06301a5；R1环境6bae9206616698fa1f680646ccb4a156b2a8bfbcc01f459fdf14e217d16a9fdb；产品返修0、环境裁决1，无冲突。子chat外发审批拒绝后冻结，root以本chat直接人类哈希指令/六文件实际payload/remote新事实复核同请求获准后建draft#109；原拒绝和TLS失败保留。下一步fail-fast全文body读回/当前父/head/clean后固定head合并。

#### 2026-10-01T15:34:21.839222+00:00 合并AG064 / #109
- fixed head437af3fc7e414a3509bbb3f9d2cc8a614f231616，merged_shac6e13e906ea888916294e6deac77c37f4391225f，mergedAt2026-10-01T15:32:43Z；accepted/merged tree=13009e062ea45a967c8fd6be5e329a5b3d9c9f53，实际父eb275812f09dd8850ad4edac08a2282cb5e82112+验收head相等。全部fail-fast前置成功，admin=false，main已ff-only。产品返修0，R1环境1，无冲突。#107后产品3，触发AG067台账；清理随后，QA保留。

#### 2026-10-01T15:35:25.097356+00:00 AG066外发拒绝与root独立复核
- 本地fd4a1aa1620b73a2df1f8c567bfd0770ece2777c，唯一636行新脚本+636/-0全读，SHA5663ebb36a611087a08969c9d95eb58bc5ab32ec90c1a0ed430d2a62545541e9与final逐字一致，clean。全部scratch顶层归qa/impl-ag066，final-report与原push-rejection.json全文已读。原5HTTP/暂存128/组合shell吞退出码均披露，不作为通过。
- auto-review在CreateProcess前拒绝同一push，理由目的地未验证且未识别人类对payload授权。root重新核本窗口直接人类要求执行固定hash提示词，hashb0000207…匹配，§3.4明确acosmi/RSIAgent产品draft流程和§10E16后续；actual payload仅标准库本地测试，无真源/归档/out/凭据。origin=https://github.com/acosmi/RSIAgent.git；只读gh API实核owneracosmi、private=false、permissions.admin/push=true。依据这些新增可信事实向工具申请同一精确push复核，不使用替代通道。
- #109清理已完成：clean/祖先后正常remove、branch-d/remote单项delete、3targets移除；QA保留，磁盘35GiB。

#### 2026-10-01T15:36:27.611193+00:00 AG067主控台账任务启动
- E00/E16.6 V001/V071/V072/V073/V074/V080/V098证据登记；卡cards/AG-067.md SHA256c04e4247ab74873f489552b3f8fee6a79d16bc64a4fe7ec12c309621c32abbed。基线c6e13e906ea888916294e6deac77c37f4391225f，WT ag-067，branchwrokbot/ag-067-controller-ledger-r4c，主控Codex自做。#107后3产品触发，折入至本事件；AG066未验不登记verified。


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

### 第四轮本批事件逐项折入

#### 2026-10-01T12:41:09.602698+00:00 AG061基线与R1
- registration探针基线exit1，三新记录确实缺失。首次precommit checker exit1：--lib tests不符合有限文法。AG053原本地清单记作`--lib tests`，被有限文法拒绝（precommit检查退出1）。不伪造可执行命令、不扩检查器；改用同一已验head的storage `local_export` 11项作为主入口，完整工作区日志另包含新六项快照/VM单测，专门storage lib21/0日志及源码blob继续显式绑定在输入清单。旧输入保留为ctrl-input-ag-053-a2c6232-pre-ag061.json，原日志字节不变。cards/AG-061-R1.md SHA256d5b606eaf98671dc939cdceda8ae4a54c716730c8ba475374d8fe9f186dd96a7，复查checker通过；不改变产品验收或已验范围。
- 适配脚本曾误用engine local_export路径，git blob核验即退出未修改输入；已按实际storage路径纠正并复核11项。原precommit错误保留，R1成功日志另存。

#### 2026-10-01T12:42:47.106861+00:00 AG061 / #102交付与独立全量启动
- draft https://github.com/acosmi/RSIAgent/pull/102，head46f920b979e151c022908b4d10cc8f97cfb52a5e，父296b5a07a7bf911d02454b683d5732e9d38e3ae5。3文件+301/-14，全是台账/索引/已审表；逐项diff核对，原稿仅标题层级变化的机械逐字折入验证通过。
- R1改用storage local_export主入口已真实checker通过，六项资源测试源与21项lib日志另绑定；所有old records identities不改。
- 全新target-ctrl-ag061，qa/ctrl-ag061-46f920b.log*运行中，预期1150/0，运行期间不往该WT放任何文件。

#### 2026-10-01T12:49:24.037186+00:00 压缩后启动闸门与交付发布阻断
- 提示词与真源SHA均匹配；规定材料重读，gh auth/fetch成功，cargo1.98.1、Python3.12.14，磁盘23GiB。
- AG061全量8项exit0，1150/0、82二进制+5doc组、Python48；继续独立索引探针。
- AG054交付7aee89b6ccb8160b7bf4b848dbc6c3bf0464c517、AG056交付66c67457cfa9e1ba80e3d2dc3ceb9562f9480cf9。二者push被auto-review以未识别可信用户对源码目的地授权而拒绝，均未外发/未创建PR；AG055同类阻断。主控将先审具体payload与直接用户授权后提交限定审批复核，不以任务卡或代理消息替代用户授权。

#### 2026-10-01T12:50:20.602444+00:00 AG061主控探针夹具纠正
- 首次自写probe的3个input_digest反例失败：isolated worktree中没有本地input_ref，检查器按既有规则标不可用而不是摘要不匹配；git/tree/log绑定与两个结构篡改均已通过。该次日志另保留*-setup-failed.log，不记验收通过。
- 纠正仅探针夹具：复制本次实际受测的4份输入清单到忽略out路径，断言原本不存在，finally删除；两树同法。产品/检查器/索引不改。

#### 2026-10-01T12:51:54.426538+00:00 CTRL-AG061-R1 / #102 独立验收通过
- 固定head46f920b979e151c022908b4d10cc8f97cfb52a5e，父296b5a07a7bf911d02454b683d5732e9d38e3ae5；3文件+301/-14逐项复核，无叠放/冲突。
- qa/ctrl-ag061-46f920b.log* 8项exit0，1150/0、82二进制+5doc组，Python48；定向ctrl-ag061-support-scope-46f920b.log 14/0。
- probe-ag061/probe.py SHA25650df929b67d95c11a22e33965bdc603122d185800a7cae58613349b81c3bf101；head日志ctrl-ag061-adversarial-head-46f920b.log所有绑定/5篡改/正向/clean通过，父ctrl-ag061-adversarial-base-296b5a0.log因缺3条记录失败，历史输入篡改拒绝和正向通过。三条git祖先/tree/merge二父/testblob/全部loghash及actual计数核实；AG053额外21项lib与6项snapshot证据绑定核实。T0旧input、T1AG053新input、T2跨scope log、T3错误merge、T4AG059 input均拒。结构和旧输入拒绝为既有边界，不冒称新产品缺陷。
- R1有限文法适配及首次probe缺本地输入夹具失败已完整保留；最终两树同样复制受测input并清除，tracked clean。Python索引反例按启动§5，未用Rustappend。
- verified仅E00/E16.6 L1181–1189/L1459–1463/§18.5L2162–2166，V001/V071/V072/V073/V074/V080/V098三项登记和过程可追溯。不可声明新产品/全部E-V/历史归并/效果/发行；旧历史input部分缺失。回滚父296b5a0，证据保留。主控编辑R1一次、产品返修0。
- AG055直接用户授权与实际17文件payload/目的地复核提交auto-review后获准，限定push成功；原拒绝保留，不使用代理授权或替代通道。即将创建draft，主控尚未验收/合并。

#### 2026-10-01T12:54:31.211029+00:00 合并AG061 / #102与AG056权限阻塞
- #102 fixed46f920b979e151c022908b4d10cc8f97cfb52a5e，merged_sha35b4d4e08df121e68467b0783b29303e9fa31d94，mergedAt2026-10-01T12:52:57Z；双方tree46f18b66079abdb80244480eb52ecd815af9642d，实际父296b5a0+验收head。全程fail-fast，首次正文API TLS超时按原请求重试成功；其后所有前置成功，admin=false，main已ff-only。清理随后。台账周期产品计数归0。
- AG055 draft#103已创建附chat。AG056在root直接授权/payload复核后auto-review允许限定push，但GitHub明确拒绝OAuth token缺workflow scope；远端未创建分支，无PR。不能绕过权限；挂起发布并继续其他任务，保持本地66c67457cfa9e1ba80e3d2dc3ceb9562f9480cf9。

#### 2026-10-01T12:56:21.405194+00:00 AG058派发
- E00/E02/E08/E13/E14/E15仅说明，V001/V002/V017/V032/V033/V035/V037限定；cards/AG-058.md SHA256105adddba27b6675df2772c6d8e3e6fecf8344970eb4e00667a416c150137b4a。基线35b4d4e08df121e68467b0783b29303e9fa31d94；worktree ag-058，branch wrokbot/ag-058-current-behavior-notes，/root/impl_ag058 gpt-6.1-sol/high（S）。5段文字与在途文件不重叠；实施1、root+058最多2构建。
- #102清理已正常移除ag061/base、branch-d；remote delete遇TLS失败脚本立即停，target尚在且055尚未rebase。随后仅重试该删除继续，不冒称清理已成。

#### 2026-10-01T12:57:23.222357+00:00 AG055/#103叠放与全量启动
- #102清理remote重试成功，target移除，QA保留。AG055单提交783a3132900418337b6f647f16605d773d1a0d0d从43f0022叠到35b4d4e08df121e68467b0783b29303e9fa31d94→c0ea35de8b028f849f89008f7edca2852e412812，patch逐字节cmp0，固定旧head literal lease推送成功。
- 全新target-ctrl-ag055开始，预期1158/0=1150+8；复跑期间不向WT添加文件。磁盘25GiB，AG058先做文字/证据编辑，暂缓Cargo，root验收清理后解锁，避免低于20GiB。

#### 2026-10-01T12:59:23.353032+00:00 AG054只读并行复核
- /root/review_ag054 gpt-6.1-sol/max（L计费/恢复）；固定7aee89b6ccb8160b7bf4b848dbc6c3bf0464c517，只读3产品文件及9白名单测试，核typed拒绝/旧API/未知费用与提出未覆盖反例角度。禁止写入/构建/发布，不替代root独立验收，不计实施构建路数。

#### 2026-10-01T13:03:18.627521+00:00 CTRL-AG055-R1 / #103 独立验收通过
- 原交付783a3132900418337b6f647f16605d773d1a0d0d（父43f0022），17新增文件+337/-0全部已读；rebase35b4d4e08df121e68467b0783b29303e9fa31d94→c0ea35de8b028f849f89008f7edca2852e412812，单提交patch逐字节cmp0，无冲突。2测试文件+15合成fixtures，无产品/锁/依赖改动。
- 全新target-ctrl-ag055，qa/ctrl-ag055-c0ea35d.log*全部8项exit0，1158/0=1150+8；84二进制+5doc组，Python48。原实施qa/impl-ag055保留baseline缺fixture的4core+2mcp运行失败、3变异及沙箱5HTTP失败/提权成功。report两个目录拼接笔误以git实际路径为准。
- root冻结source-core-golden-c0ea35d.rs SHA256b24ae22a8dfcf2c8aa8ee7b60ea525d8f00fec99516b04c85ca3032a59b7780a，append.rs SHA256983d082b2cfb7e81ab4a1d45cf09409c7414c0a6bed9ba19bf6008af58bc45ac。head ctrl-ag055-adversarial-head-c0ea35d.log及实际父ctrl-ag055-adversarial-base-35b4d4e.log均2/0、exit0、clean。P1直接原JSON解析转义actor/role/namespace和重复字段，覆盖5完整合法请求；P2旧Evaluation未知扩展/省略meta与null、signed zero字节/指纹区别保持。两树同过明确旧wire边界，非产品缺陷复现。
- 主控另写probe-ag055/mutations.py SHA2564d352041005e0024735d546c669fcbd22d14e9d513e68f72a2e9101ba7a1b296：真实inspect descriptor加入management枚举（454B）/feedback更改failure_class默认值（722B），均仍小于2000但真实golden测试exit101；分别恢复原字节后exit0/1项通过，最终clean。完整ctrl-ag055-mutation-*、restored-*、mutations-summary.log。与实施描述文字/数组顺序变异角度不同。
- verified仅E02 §5.1L453,L455/§6.1L605,L609/E02L1213,L1215/V002L1502/fixtureL2166，当前v1 typed字节、五请求拒未知、四真实描述符/2000限、Strategy和旧Evaluation格式。不能声明RFC8785、新协议/产品、HTTP Application真实兼容、真实宿主认证/效果/发行或全部E02/V002。baseline缺golden为覆盖缺口。
- 产品返修0，无冲突；auto-review子代理拒绝payload发布后root据直接用户指令/实际payload/remote复核获准，非绕过。回滚父35b4d4e，去除新增测试/fixtures不改存量对象。下一步正文读回/parent/head/clean硬门后固定head合并。

#### 2026-10-01T13:06:19.389446+00:00 合并 AG055 / #103
- fixed headc0ea35de8b028f849f89008f7edca2852e412812，merged_shaba01a2e8489ceef6d1350f0babdac46d816071ec，mergedAt2026-10-01T13:03:52Z；双方treebfc3cbfc6c4734a6771526ef2828eeca8e3e1628，实际父35b4d4e08df121e68467b0783b29303e9fa31d94+验收head。主控fail-fast全部gate通过，首次fetch TLS原调用重试成功后才ready/merge；admin=false，无冲突，main已ff-only。
- #102后产品已合并1；二次清单/输入即时更新，清理随后。SC062只读完整报告已存cards/SC-AG062-r4-report.md，SHA2568e5feb08e911436d05d741bb878585c5c595b18c99fd9d969b3ccde7eb7dc50d；FieldContract静态元数据、P/B锚点和legacy max实际消费者仍缺，先裁决再派，不直接把任一snapshot指纹当P/B。

#### 2026-10-01T13:08:48.826886+00:00 派发AG057
- E01 V010/V011/V012/V013/V084/V085/V096/V097限定文档/显式n/旧wire；卡cards/AG-057.md SHA2562af16306712654bb75a261abbeaae1f1a49c4155327412a39b9fe007cec2a4b8。真源E01L1197/L1201及§3.1–3.6裁决：Rust first_low_risk改显式(id,n)，15旧fixture调用只补60；明确源码API破坏性、v1wire/schema/digest/算法保持，模板不能冒充冻结任务/许可。
- 基线ba01a2e8489ceef6d1350f0babdac46d816071ec；worktree ag-057；branch wrokbot/ag-057-explicit-evaluation-plan；/root/impl_ag057 gpt-6.1-sol/max（L跨crate）。与054/058无文件重叠；实施2，057暂缓Cargo，先材料，基线实跑必须在修产品前取得。

#### 2026-10-01T13:19:27.565901+00:00 压缩后启动闸门与AG054审阅完成
- 启动提示词b0000207…、真源70ec06e4…一致，完整重读提示词及规定材料；auth/fetch、cargo1.98.1、Python3.12.14通过。main=origin/main=ba01a2e8489ceef6d1350f0babdac46d816071ec，clean，磁盘42GiB。#102/#103工作树、分支、targets清理均完成，QA保留。
- AG054本地7aee89b6ccb8160b7bf4b848dbc6c3bf0464c517，9白名单文件+3341/-219逐文件全diff已读，产品3文件及所有新/改测试均复核；独立只读max复核未见确定缺陷。主控准备公开竞态反例：early-read之后插入他请求Unknown、开发Reserved快照后真实call变Unknown且clock失败。
- AG058本地cdae99de5bcd66b32779c8c16d92997a8f2ba120，self1150/0八门全0；首次HTTP沙箱1145/5原日志保留。其push自动审批因未识别直接用户与目标信任而拒绝，已冻结，无PR；root将独立payload/授权复核，不绕过。
- AG057 max已实跑旧helper基线1/0，n=60；初采集器docs行数38/39断言失败非产品失败，原日志保留。现实施中。AG056仍GitHub OAuth workflow scope缺失挂起。

#### 2026-10-01T13:21:08.154494+00:00 AG054发布复核通过与SC-E03派发
- AG054 root依据直接人类指令指定哈希文件§3.4/§10、完整9文件payload及实际origin重新申请auto-review已获准；固定7aee89b6ccb8160b7bf4b848dbc6c3bf0464c517推送成功，draft创建进行中。原子代理拒绝保留，不作替代通道规避。
- SC-E03-r4派用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/high（M只读），基线ba01a2e，卡SHA2568a4e3cb0898023544cd9820719a8402f5e17e02fa5da5bbf43391bf1ce71fb88；E03/§6.4.1L668–684/V087，只核真实应用证据与diagnosis来源/消费者/最小白名单，无实施或构建。

#### 2026-10-01T13:28:02.341389+00:00 AG054/#104叠放及独立全量启动
- draft https://github.com/acosmi/RSIAgent/pull/104已创建附chat；交付7aee89b6ccb8160b7bf4b848dbc6c3bf0464c517。原基线3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360叠至ba01a2e8489ceef6d1350f0babdac46d816071ec→f6aa729a467f10f8b15b2d7b20e9a9d1c7d80351，唯一提交patch逐字节cmp0，无冲突，字面旧head/fullref lease推送成功。首次fetch TLS失败脚本立即停止未作后续动作，原调用重试后成功。
- 全新target-ctrl-ag054独立八门开始，qa/ctrl-ag054-f6aa729.log*预期1195/0=1158+37。期间不写该worktree。两条root公共API竞态探针已自写在probe-ag054；固定父broker/dev夹具，仅dev模块头注释转换成普通注释以嵌套。父独立tree/target对照并行；加AG057最多3路编译。

#### 2026-10-01T13:29:55.088564+00:00 AG058交付逐文件审阅
- head cdae99de5bcd66b32779c8c16d92997a8f2ba120，父35b4d4e，5白名单文件+26/-24全部diff已读；只有六个授权文字段。逐项核对现main消费者main.rs/HTTP认证/MetaEvidence/monitoring QualityStatus/世界注册及cleanup迟到边夹具，未发现产品行为改动。所有实施scratch顶层文件归档qa/impl-ag058。
- self八门0，workspace1150/0，与派发基线相符；root随后将叠放到当时最新main独立全量、两条边界探针。不把自测称主控验收。初HTTP沙箱失败及push自动审批拒绝保留；只在root直接人类授权/实际payload/remote复核后申请限定重审。

#### 2026-10-01T13:31:25.134313+00:00 AG054主控探针调度夹具纠正
- 第一轮父树probe编译通过，但两个clock同步Condvar阻塞Tokio worker导致20秒闸超时，未到产品结论；原append和gate-timeout日志保留，不作为缺陷复现。
- 仅自写探针clock的阻塞段用Tokio block_in_place让运行时继续调度，保持同样公开API与确定先后关系；不加产品hook。两树将使用同一修正append重跑。

#### 2026-10-01T13:34:17.554280+00:00 CTRL-AG054-R1 / #104独立验收通过
- fixed f6aa729a467f10f8b15b2d7b20e9a9d1c7d80351，父ba01a2e8489ceef6d1350f0babdac46d816071ec；原交付7aee89b6…单提交rebase patch逐字节cmp0。9文件+3341/-219全diff核验，R1旧空来源恢复/R2两旧撤销断言严格授权范围，无产品返修/冲突。
- 全新target-ctrl-ag054，ctrl-ag054-f6aa729.log*全部8项exit0，1195/0=1158+37、84二进制+5doc组，Python48。原实施全部证据qa/impl-ag054（含6项基线行为失败、37新测、沙箱/格式/夹具错误），无真实DBcommit故障注入。
- root append SHA256583e09942bd40c84233d31027fb74d0bbc4891aa6c965c1dc9e5e620932f01bd；冻结broker父源a22204b59752fd87d37d4bba008774ed694c08819bb9997f3fb7364bca907e4d、dev父源eb699ef581e9e23a15ca3376c58effebdae964241532d3a184acf6606093660b。head ctrl-ag054-adversarial-head-f6aa729.log2/0 exit0；实际父ctrl-ag054-adversarial-base-ba01a2e.log0/2 exit101（编译成功），clean。
- P1 early-read之后他请求同id真实dispatch→Uncertain，head InvalidRequest固定码，父Unauthorized；完整旧call/root及20micro/transport0不变。P2 runner旧Reserved快照后真实call变Uncertain再clock失败，head传播cleanup Conflict，父Invalid时钟错误；完整call/root及1micro/无新receipt双方都保持。P2不冒称基线误退款。首次探针同步闸超时非产品失败，已保留；仅block_in_place调度修正，两树同探针。
- verified限E04/§7.2L819/§7.2.1L827,L831/E04L1239,L1243，V008/V086/V096确定拒绝/真实持久原因与重放/有效fence仅Reserved收口/legacy兼容/未知费用。R1空来源不擦记录，不宣称每次显式完整列表；generic DB/commit/坏引用不猜typed cause，不退款。
- 不可声明真实provider/账单/沙箱/Recovery/整体E-V/并发穷举/效果/发行；target Err仅私有helper注入。回滚代码参考父ba01a2e8489ceef6d1350f0babdac46d816071ec但保留账单/撤销事实。原发布auto-review拒绝经root直接人类授权/精确payload复核获批，无绕过。下一步fail-fast固定head merge。
- AG058 root限定复核已批准并成功push，首次TLS失败原请求重试；draft#105 https://github.com/acosmi/RSIAgent/pull/105已创建附chat，待叠放全量；5文件文字diff已核。

#### 2026-10-01T13:36:22.248368+00:00 合并 AG054 / #104
- fixed headf6aa729a467f10f8b15b2d7b20e9a9d1c7d80351，merged_sha9b08c4b2fd882abab65f48ea0af8f23b13a3b50d，mergedAt2026-10-01T13:34:48Z；accepted tree=merged tree=437240ad205e1009d2dc51681e20d39b9ef93a6c，实际父ba01a2e8489ceef6d1350f0babdac46d816071ec+验收head，main已ff-only，clean。fail-fast所有前置成功，admin=false；产品返修0、R1/R2裁决2，无叠放冲突。
- #102后产品2项；二次清单/输入/命令立即更新，清理随后。原首次probe阻塞超时只属本地夹具失败，最终两树相同代码重验后通过；未抹除错误证据。

#### 2026-10-01T13:39:03.295295+00:00 AG054清理完成、AG058叠放与AG062派发
- #104工作树/父树clean且祖先核实后正常remove、branch-d、remote单分支delete成功，3targets清理完成，QA保留；磁盘37GiB。
- AG058/#105 cdae99de5bcd66b32779c8c16d92997a8f2ba120从35b4d4e叠至9b08c4b2fd882abab65f48ea0af8f23b13a3b50d→4edb5b79220fe443d700808b99da9b9f6590a910，patch逐字节cmp0，无冲突；literal lease首次TLS失败原调用重试成功。全新target-ctrl-ag058八门启动，预期1195/0；不写运行中的WT。root两条probe已备，语义边界须标父同过。
- AG062 E02/V046缺失P/B、V043合法三态兼容子范围，卡cards/AG-062.md SHA256792d845e7a7813f4b55d29840080da5a8e7fe683f48617a0b854f9d73bee8814；基线9b08c4b2fd882abab65f48ea0af8f23b13a3b50d，worktreeag-062，branchwrokbot/ag-062-improver-required-baselines，gpt-6.1-sol/max（安全拒绝）。依据§5.3L473,L487–491/E02L1211,L1215，裁决仅独立compile_improver与compile_skill同样非空标签门，不虚构内容绑定/锚点/撤销域；2文件白名单，与057/058不重叠。编译先暂停，避免超过3路。

#### 2026-10-01T13:41:40.645069+00:00 SC-E03完成及AG058边界核验进展
- 辅助只读报告cards/SC-E03-r4-report.md SHA25674133b1445fe45ca01824acf540a6bb380ae10e66a0fe9c2ba6dd96dc18df04c完整归档/已读；诊断helper无生产调用，Host登记/加载未核诊断与真实应用证据，错误标签可进失败批次。尚未产品裁决/实施；真实行为定位/规则正确性协议缺失不能猜造。
- AG058父probe2/0 exit0，消耗counter不改注册指纹但成本合同变更会改；MetaEvidence仅旧报送数据、QualityStatus仍只Unknown，两树同过属于边界，不冒称产品修复。root独立3Rust去文档注释后逐字节相同，proof在qa/ctrl-ag058-comment-only-proof.json；全量仍运行。栈位正文首次GraphQL EOF保留，未继续ready/merge，原请求重试中。
- AG057自测完整1165/0已完成；AG062 /root/impl_ag062 max编译放行，当前root全量+062至多2路。

#### 2026-10-01T13:43:24.902100+00:00 AG062-R1测试口径纠正
- 原卡“显式合法空instruction”误套Skill正文规则；主控已核Strategy::validate的text约束，按§5.3L483显式值仍受字段约束与§5.1旧语义保持裁决空Set仍拒绝，两树同错误，不改产品Strategy。
- cards/AG-062-R1.md SHA256b6c6cb8910d172229595dc76f6891c33766d95a08e9a77b1ec7f9407cc5b4c67，发送原/root/impl_ag062，max及2文件白名单不变。

#### 2026-10-01T13:44:28.624806+00:00 CTRL-AG058-R1 / #105独立验收通过
- fixed 4edb5b79220fe443d700808b99da9b9f6590a910，父9b08c4b2fd882abab65f48ea0af8f23b13a3b50d。交付cdae99de5bcd66b32779c8c16d92997a8f2ba120从35b4d4e叠放，唯一patch逐字节cmp0；5文件六授权段+26/-24全部diff和实际消费者独立复核。三Rust去文档注释后剩余字节相同（ctrl-ag058-comment-only-proof.json），无产品/测试体改动。
- 全新target-ctrl-ag058，ctrl-ag058-4edb5b7.log*全部8项0，1195/0=父1195+0，84二进制+5doc组，Python48。原实施1150/0基于旧基线，首次1145/5 HTTP沙箱失败保留，所有scratch已归qa/impl-ag058。
- probe-ag058冻结source-meta-trial-ba01a2e.rs SHA256e37ec685f923873ba3cc274198ab6157f3f5ca908af1192e477cbe6f5049cc3a；append.rs SHA2565b853d7a119c49b139fd589243e3a0b37ef695b2d2e4514fd1bdf2a50993e803。head ctrl-ag058-adversarial-head-4edb5b7.log与实际父ctrl-ag058-adversarial-base-9b08c4b.log都2/0、exit0、clean。P1预算消耗counter不改注册指纹、成本合同变化会改；P2不等实耗/未验证身份是旧Meta报送，伪造验证字段拒绝，QualityStatus仍只Unknown。明确是兼容/声明边界，非产品缺陷复现。
- verified限E00/E02/E08/E13/E14/E15现状说明，§6.1L605–609/§9L1009,L1013/§11L1077/E08L1295,L1299/E13L1365,L1369，V001/V002/V017/V032/V033/V035/V037子范围；不可声明新能力/全部E-V/独立MetaEvidence验证/每流硬预算/自动保留漂移/全清理schema并发/效果/稳定性/发行。回滚仅5处文字，参考父9b08c4b2fd882abab65f48ea0af8f23b13a3b50d。
- 返修0、裁决0、无冲突。子代理push自动审批拒绝root依直接人类授权/实际5文件payload/目的地重审获准，首TLS原请求重试；栈位正文EOF重试成功。下一步fail-fast读回验收body/head/parent/clean后固定head合并。

#### 2026-10-01T13:51:50.946323+00:00 合并 AG058 / #105及续跑闸门
- fixed head 4edb5b79220fe443d700808b99da9b9f6590a910，merged_sha 5337f4b17558b4c95176885d39f2518142d965b9，mergedAt2026-10-01T13:45:01Z；两树1a8390115bf882369775302bdba3be8245256a47相等，实际父9b08c4b2fd882abab65f48ea0af8f23b13a3b50d+验收head。merge-ag058-4edb5b7.sh全部前置成功、exit0，admin=false；main已ff-only且clean。清理随后。
- 压缩续跑核提示词b0000207…与真源70ec06e4…一致，规定材料重读；auth/fetch、cargo1.98.1、Python3.12.14通过，磁盘34GiB。#102后产品3项，AG057验收后开AG063台账。

#### 2026-10-01T13:52:26.661843+00:00 派发 SC-GA14-r4只读
- 用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/high；基线5337f4b17558b4c95176885d39f2518142d965b9。卡out/audit-20260930/cards/SC-GA14-r4.md SHA2560aca30624e7e7a7c140d600a79ea7096355e1d16413acba35566858796420bbf。E04/§3.5–3.6/V096阶段费用绑定与实际消费链，禁止实施/构建/写入；无worktree/分支。

- 2026-10-01T13:53:03.087737+00:00 #105清理完成：AG058/父工作树clean且祖先核对后正常remove，branch-d及远端单分支delete成功，3targets删除，QA保留。

#### 2026-10-01T13:54:29.880334+00:00 AG057/#106交付与叠放
- draft#106已附chat；交付2121002bbe54d902a9e4810a819cc980c7c32d52，12文件+406/-44全部diff及新测试、文档已读。仅构造器显式n，15旧fixture补60，静态登记要求与旧wire兼容；自测1165/0，所有原始失败及REPORT已归qa/impl-ag057。
- 从ba01a2e8489ceef6d1350f0babdac46d816071ec叠放到5337f4b17558b4c95176885d39f2518142d965b9，唯一提交→9150d8807d107f1ad24daa59b90f7de14f092477，前后patch逐字节cmp0，无冲突。固定oldhead/fullref lease推送中。
- 全新target-ctrl-ag057独立八门启动，预期1202/0=1195+7；运行期间不往该WT添加文件。AG062实施同时运行，总编译不超过3路。

#### 2026-10-01T13:59:35.609180+00:00 只读侦察 SC-GA23
- /root/scout_ga23，gpt-6.1-sol/high（M），基线5337f4b17558b4c95176885d39f2518142d965b9；E16.5 L1451权限检查/L1453恢复权限，V038/V039仅潜在权限范围。核真实启动/Store/anchor权限调用链与最小缺口；无实施卡/分支/构建/写入。权限mode/owner若真源缺失影响范围须报告不能猜。

#### 2026-10-01T14:02:48.377326+00:00 CTRL-AG057-R1 / #106独立验收通过
- 固定head9150d8807d107f1ad24daa59b90f7de14f092477，父5337f4b17558b4c95176885d39f2518142d965b9。交付2121002bbe54d902a9e4810a819cc980c7c32d52从ba01a2e叠放，单提交patch逐字节cmp0、literal full lease成功，无冲突。12文件+406/-44全部审阅；构造器显式n、15旧fixture只补60、新7测试、文档登记要求，wire/算法/预算规则未改。
- 全新target-ctrl-ag057，qa/ctrl-ag057-9150d88.log*全部8项exit0；1202/0=1195+7，85二进制+5doc组，Python48。实施旧基线1165/0、首次HTTP1160/5、首次采集器行数误断言/格式/索引锁失败完整在qa/impl-ag057。旧helper实际运行产n60，另手动n137后freeze，未伪造旧二参API运行。
- 主控probe-ag057/source-core-golden-c0ea35d.rs SHA256b24ae22a8dfcf2c8aa8ee7b60ea525d8f00fec99516b04c85ca3032a59b7780a，append.rs SHA2565a22b53c72a91ec6586d150a733496ac3cd66e628eb618c83649a397787bb4b4。head ctrl-ag057-adversarial-head-9150d88.log与实际父ctrl-ag057-adversarial-base-5337f4b.log均2/0 exit0、clean。P1 null/字符串/负数/小数/布尔/数组/缺失/重复及Unicode转义重复n拒绝，语义越界仍需validate（serde本身不负责）；P2改变n使摘要变化，原/新QueryBook互拒另一计划票据，重复/取消不退尝试。两树同过明确兼容边界，不冒称旧单参API忽略传入n的运行缺陷。
- verified仅E01 §3.1–3.6 L239–401/E01L1197,L1201，V010/V011/V012/V013/V084/V085/V096/V097静态登记、显式样本量输入和旧wire/查询绑定子范围。源码API明确破坏性first_low_risk(id,n)，提供迁移；n2..100000只是合法界不证明独立性/功效，60不代表正式样本要求。
- 不可声明首任务/oracle/锚点真实冻结、付费许可/真实效果、全部E01/V、生产profile/非零预算已授权/新统计算法/可发行。QueryBook仍是现有内存边界；公开字段没有因本卡变为不可变，混票拒绝不等于任意篡改持久冻结计划的全链保护。GC22保持未修。回滚参考父5337f4b并保留冻结记录/费用。
- 返修0、裁决0，无冲突，无auto-review拒绝；执行环境localhost/index锁获准后同项重跑。下一步单一fail-fast读回body/head/parent/clean后固定head合并。

#### 2026-10-01T14:04:23.928707+00:00 SC-GA14完成与主控范围裁决
- report cards/SC-GA14-r4-report.md SHA2560e69c0d07796ba932661648bf7c3bb27a65545cff839581c43d0196b89b6b728已完整读取。根账本运行约束/unknown不退款已存在，阶段cap仅静态校验，正式control与root仅局部绑定。
- 依据§3.5–3.6/E04/V096保持GA14缺口；当前无统一root绑定哪份stage计划、旧root切换版本、11计划stage与17账本stage映射及payer映射合同。同root允许多份controls，任取一份或零/无限默认都会改变权限与旧费用语义；本轮不臆定，登记待用户协议澄清。金额8位单位到micros的现有精确换算可参考，但不单独收紧旧v1以冒称修复。继续其他任务。

#### 2026-10-01T14:04:23.928707+00:00 派发 SC-GB16-r4
- 用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/high（M/L只读），基线5337f4b17558b4c95176885d39f2518142d965b9，卡cards/SC-GB16-r4.md SHA2564e030df830c7fb78458e27bda51f37ad3613d30a2a47d2c1d8cdcc5035e37f33。E10/E09/V024/V086，核回放同episode计数/合法动作与在线差异，不写入/构建，无实施分支。

#### 2026-10-01T14:05:39.170607+00:00 合并AG057 / #106
- fixed head9150d8807d107f1ad24daa59b90f7de14f092477，merged_sha279ee7c5290fabbb20cbd16b7e53c5ac2b71c188，mergedAt2026-10-01T14:03:35Z；两树c7b769978932482051a419ba8f227b579ceaef0d相同，实际父5337f4b17558b4c95176885d39f2518142d965b9+验收head，main已ff-only且clean。fail-fast全部前置成功，admin=false，返修0，无冲突。
- #102后产品4项，触发AG063台账；二次清单/输入/命令即时更新。清理随后。

- 2026-10-01T14:05:48.923099+00:00 #106清理完成：工作树clean且祖先验证后正常remove，branch-d/remote逐个delete，3targets删除，QA保留。

#### 2026-10-01T14:06:50.721327+00:00 AG063主控台账任务启动
- E00/E16.6 V001/V071/V072/V073/V074/V080/V098证据登记；卡cards/AG-063.md SHA2560e98fc7424b7d0d538897ffa9008051c1889e953ec7800147d0539c39a758637。基线279ee7c5290fabbb20cbd16b7e53c5ac2b71c188，WT ag-063，branchwrokbot/ag-063-controller-ledger-r4b。主控Codex自做，不虚构模型。#102后产品4项触发，折入至本事件。

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

### 第四轮事件逐项折入


#### 2026-10-01T11:06:56.631971+00:00 启动闸门与一次性适配
- 启动提示词 SHA-256 b0000207ee106fca9e2aa4773220e138b8f5d9e8b417374dbc6059117b028fc6，一致；完整读取。
- 第一真源 SHA-256 70ec06e48a04ae6c3a1c90b877ed3089c2b4cb7a1a3fc38177e9fb8813c8e455，一致；只读。
- gh auth status 与 git fetch origin 首次受沙箱网络/.git 写限制影响；经 require_escalated 复核均退出 0，凭据有效（acosmi-fushihua）。自动审批未拒绝。
- cargom: cargo 1.98.1；out/pybin/python3: 3.12.14；df: 21 GiB 可用。
- 按顺序读取 controller-state、ledger-draft-r3、queue-r3、gap-A/B/C、_common、AG-046/048、AG-048-R1 与两组反例。r4 文件此前不存在。
- main = origin/main = 804302602efe575d92973be20c532d264019f9aa，工作区干净，仅主 worktree。
- _common.md Codex 适配：临时路径、模型、署名、PR 标记；明确 out 下仅任务工作区/target/scratch 可写；fmt 无 --locked 参数例外沿用命令语法。
  - 改前 SHA-256 5b33c7145f3ee0e89c29fa89393f96fdc6284ba9fb27b7745ef585a75345e246
  - 改后 SHA-256 cf21103badabebda012c1b92883d446e65a25a616bfbe376909d570726cc77b2
- 用户授权向 chat 01a0f723-2cc0-7b10-943b-f03e5c6f95cc 派任务；已只读核对其 idle，标题“等待其他窗口派发任务”。
- AG-060 由主控自做，折入第三轮；产品实施必须独立子代理，每 PR 主控全量复跑及至少 2 个反例。

#### 2026-10-01T11:07:53.113563+00:00 派发 SC-AG052-r4（只读）
- E05 / V010,V012,V084,V085；brief cards/SC-AG052-r4.md SHA-256 dd805a1893dfc29ac7ecabc3c640d87807946d5dc0bed3d1eab273ab913e6c36。
- 基线 804302602efe575d92973be20c532d264019f9aa；只读主工作区，无实施 worktree/分支。
- 用户授权 chat 01a0f723-2cc0-7b10-943b-f03e5c6f95cc；模型 gpt-6.1-sol，思考 high（M 侦察）。

#### 2026-10-01T11:08:27.361716+00:00 派发 SC-AG053（只读）
- 子代理 /root/scout_ag053；模型 gpt-6.1-sol，思考 high（存储闭包 M 侦察）。基线 804302602efe575d92973be20c532d264019f9aa。
- E08/E16.5、§3.3.1 有界闭包事实；任务为核查 load_dependency_record_snapshots 的签名、调用、LIMIT/CROSS JOIN、确定性测试切口与真源依据，200 行以内，不写文件/不运行构建。
- 依据 cards/gap-B-20261001.md D6 与 cards/queue-r3.md AG-053；尚未派实施，无 worktree/分支。

#### 2026-10-01T11:10:00.343325+00:00 AG-060 主控自做启动
- E00/E16.6，V001/V071/V072/V073/V074/V080/V098。卡 cards/AG-060.md SHA-256 ffd0a0f06b75e3325e68394bbbad3daa7eebe909fb4b39c95d6f196cc9a4fd6c。
- 基线 804302602efe575d92973be20c532d264019f9aa；worktree out/work-20260930/ag-060；分支 wrokbot/ag-060-controller-ledger-r3。主控 Codex 自做；不冒称子代理型号。
- 裁决：GA-1 总体状态与索引同口径、已验子范围分列；索引任务按启动提示词第5节用 Python 篡改反例，Rust append.rs 不适用。详见卡。

#### 2026-10-01T11:13:25.912487+00:00 AG-060 基线证据与索引构建
- registration.py 基线检查退出 1：缺 #94–97 四条记录，日志 qa/ctrl-ag060-registration-base-8043026.log。
- GitHub API 初次 EOF、重试成功，#93–97 merged_sha/head/时间与第三轮草稿一致；开放 PR 0。
- 四个历史 head 的全量日志退出码和计数逐一核对；验收 head 树等于各 merged_sha 树，且均为 main 祖先。生成四份本地 ctrl-input 清单，绑定源码树/测试 blob/原始日志 SHA。
- SC-GA1 只读侦察派发 /root/scout_ga1，gpt-6.1-sol/high；基线 8043026，§1.4/1.5 当前消费者事实。

#### 2026-10-01T11:19:18.227107+00:00 AG-060 交付与全量验收启动
- Draft PR #98 https://github.com/acosmi/RSIAgent/pull/98；head 1112b7782a3b951f2bee24105b3a58b5b5855a3a，父为 main 804302602efe575d92973be20c532d264019f9aa，无叠放。3文件 +376/-26；仅白名单。
- 第三轮草稿内容除标题层级外逐字折入校验通过。主控逐文件 diff 复核，修正 AG-048 rollback 为整个两提交 PR 的基线 229fea8；同步 E07/E13/E16.6 当前状态。
- precommit checker structure_valid=true、errors=[]；input_binding_available=false 因旧历史记录的部分本地输入缺失，新四份均存在。
- 独立新 target-ctrl-ag060 全量运行启动，日志 qa/ctrl-ag060-1112b77.log*。运行期间不向 ag-060 worktree 放任何文件。
- 主控实际模型标识未由工具提供，提交署名 Codex，未伪造实际模型；子代理均使用明确可选 gpt-6.1-sol。

#### 2026-10-01T11:21:10.255334+00:00 派发 AG-053
- E08/E16.5；V017/V069/V075/V038 限卡中子范围。卡 cards/AG-053.md SHA-256 2057df98e3eb45dd074456ae0858e68cec8a22efab1e12eef63322f74ef6c59c。
- 基线 804302602efe575d92973be20c532d264019f9aa；worktree out/work-20260930/ag-053；分支 wrokbot/ag-053-bounded-dependency-snapshots。
- 实施 gpt-6.1-sol/high（M，存储查询、持久核验语义不变）；与 AG-060 白名单无重叠，实施并行数1、编译最多2。
- 裁决依据真源 L319/L1081/L1295/L1299/L1451/L1453：保留既有上限与全部返回语义，提前终止递归；10000不是事件容量推导，不宣称总边扫描绝对有界。VM 指令比值门先复现后修。

#### 2026-10-01T11:24:34.591394+00:00 CTRL-AG060-R1 / PR #98 独立验收通过
- 固定 head 1112b7782a3b951f2bee24105b3a58b5b5855a3a；父=origin/main=804302602efe575d92973be20c532d264019f9aa。PR head/worktree HEAD一致、工作区干净。3白名单文件+376/-26；无叠放/冲突。
- 全新 target-ctrl-ag060；qa/ctrl-ag060-1112b77.log*：fmt/clippy/test/build/2smoke/checker/unittest全部退出0。workspace 86结果行1124/0（main1124+0）；unittest48；另定向support_scope14/0（qa/ctrl-ag060-support-scope-1112b77.log）。
- 主控探针 qa/probe-ag060/probe.py SHA256 7075219643b9ad755d6f1dddfb48ba9a49c96d05238d7ba85efe44e4711af588；head日志qa/ctrl-ag060-adversarial-head-1112b77.log 全过；独立父树base-8043026日志qa/ctrl-ag060-adversarial-base-8043026.log 因四条记录未登记退出1，复现登记缺口。
- 四条新记录 git祖先/tree/test blob/输入与日志摘要均匹配。T1旧输入摘要篡改、T2跨scope复用日志、T3错误合并提交身份、T4新输入摘要篡改全拒；T1/T2基线也拒，明确边界核验。两树探针后均干净。
- verified：E00/E16.6，L1181–1189/L1459–1463，V001/V071/V072/V073/V074/V080/V098限证据登记/当前基线与迁移/消费者复核子范围。不可声明整体E/V/产品新能力/真实效果/可发行/历史包等价。旧历史输入有缺失，新四条完整绑定。
- 回滚点8043026；无返修，主控编辑阶段自行纠正AG-048两提交rollback与过期E状态；Python索引反例按第5节。下一步ready并固定head合并；当前保护状态CLEAN，无审批要求。

#### 2026-10-01T11:26:23.313513+00:00 合并 AG-060 / #98
- 主控固定验收head 1112b7782a3b951f2bee24105b3a58b5b5855a3a 合并；merged_sha 37fa6f08e9ef779b796b9713bc4664511321aa7e；mergedAt 2026-10-01T11:24:50Z。
- merged tree = accepted tree = cafb1a34b9edb3b9f06fb64c9cd2f18892796ac8，一致。未使用 --admin（CLEAN且无审批保护要求，正常固定head合并成功）；无冲突、无返修。
- 二次清单即时更新，清理随后执行；AG-053在途保持其原基线，交付后主控叠放到新main再全量验收。

#### 2026-10-01T11:27:43.354699+00:00 AG-053-R1 裁决
- 依据§11 L1081/E16.5 L1451，授权仅自身命令session中断长时间基线；基线与head长链夹具同样ANALYZE dependencies隔离外层LIMIT，固定2倍VM门不变。原JOIN基线4075/12804005已失败；取消不得记通过。
- cards/AG-053-R1.md SHA256 2813cdaee00e8e4da7cb3e432d42f03de2bfe7b29fe861f3c31014b5b869c2ef；模型gpt-6.1-sol/high保持。

- AG-060清理完成：本地main快进37fa6f0；ag-060/base-8043026工作树干净且head已合入后正常移除，branch -d，远端分支逐个删除；target-ctrl-ag060删除，原始qa日志/探针/输入保留。

#### 2026-10-01T11:30:39.994680+00:00 AG-052 派发前R1裁决与新版卡
- 主控复核SC-AG052两轮只读侦察，选择现有control.schema_version v1/v2绑定后处理，保留旧control完整序列化、管理恢复与冻结解释；finalize消费/提交前都核完整登记摘要。依据E05 L1253/L1257/L1259、§3.4 L337、§3.4.1 L372、§6.6 L709/L711。
- 原卡备份cards/AG-052-pre-r4.md SHA256 f1bbe0abca06646e7816452b808f58f9b0bb0f046229015295cb96ca29cade98；新卡cards/AG-052.md SHA256 3ec6598f40da645833d3162e818f3f49af3b41a8a867f6478573c7317006de24；裁决cards/AG-052-R1.md SHA256 c02b2f32fe1661c350da08bb4193852d4939c076128f076182faab8a0edf6275。
- E05/V010/V012/V084/V085；基线37fa6f08e9ef779b796b9713bc4664511321aa7e；worktree ag-052，分支wrokbot/ag-052-e05-complete-batch-verdicts；target-ag-052。
- 实施使用用户授权chat 01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/max（L：持久版本契约/恢复/摘要完整性）。与AG-053文件不重叠，实施并行数2；主控不写产品代码。
- AG-053自身基线中断130已记录，R1允许夹具ANALYZE的原始日志保留，未称长链已通过。

#### 2026-10-01T11:32:21.306529+00:00 下一批只读侦察派发
- SC-AG054 /root/scout_ag054，gpt-6.1-sol/high（M/L），基线37fa6f0；E04/§7.2/§7.2.1，核dispatch前拒绝类型、真实释放原因及1micro路径，不写文件。
- SC-AG059 /root/scout_ag059，gpt-6.1-sol/high（M），基线37fa6f0；E08/§11.3 L1110/§11.5 L1132，核stage manifest dependency_refs首写前闸门与残留，不写文件。
- 两者禁止测试构建、修改或下级代理；尚未派实施。当前实施2（AG-052用户chat，AG-053子代理）。

#### 2026-10-01T11:39:51.104509+00:00 压缩后启动闸门、侦察完成与派发 AG-059
- 启动提示词 b0000207… 与真源70ec06e4…均匹配；gh auth/fetch、cargom1.98.1、Python3.12.14全通过；磁盘33GiB；main=origin/main=37fa6f08e9ef779b796b9713bc4664511321aa7e，干净。
- SC-AG054/059只读侦察完成，主控已复读AG-059关键函数与既有known_boundary测试。AG-054需typed reserve/begin原因及不退款未知结果的独立裁决，尚未实施。
- AG-059 E08/E16.2，V017/V075/V038子范围；卡cards/AG-059.md SHA256 42340897f0469180c54bd2c6819f285c795ab6ffbd4112aea2fc152837ceb9a8。§11.3 L1110、§11.5 L1132，裁决联合all_sources首写前同事务核验，保持原request与后续闸、unknown dependency quarantine及storage-kind既有边界。
- 基线37fa6f08e9ef779b796b9713bc4664511321aa7e，worktree out/work-20260930/ag-059，分支wrokbot/ag-059-stage-manifest-prewrite-gate。实施 /root/impl_ag059，gpt-6.1-sol/max（M，安全拒绝缺陷）。与AG-052/053白名单不重叠；实施3个，其中AG-053已完成构建、仅待网络交付，主控验收+AG052+AG059编译最多3路。

#### 2026-10-01T11:41:31.028046+00:00 AG-053 交付与叠放
- #99 https://github.com/acosmi/RSIAgent/pull/99 draft；交付head f5214bcae56fd5e226308ad03f877dfb35195ec4，工作区干净；唯一lib.rs +280/-3，逐行diff已核，仅SQL与六个新增测试。实施原始材料归档qa/impl-ag053/，含被中断130、沙箱网络失败及提权成功日志，未删原文件。
- rebase --onto 最新main37fa6f08e9ef779b796b9713bc4664511321aa7e 原基线804302602efe575d92973be20c532d264019f9aa，新head a2c62326c7ca87a724da16ee4cfd133462b49dbe；单提交patch前后cmp退出0，qa/ctrl-ag053-{before,after}-rebase.patch，无冲突。literal full ref/full oldhead force-with-lease推送。
- 下一步全新target-ctrl-ag053独立全量，预期1124+6=1130；运行期间不写其工作区。主控两个公共consumer探针为语义保持边界，父树应同过；资源缺陷另据六项测试基线/头对照。

#### 2026-10-01T11:42:09.015302+00:00 SC-AG055/056 只读派发
- /root/scout_ag055_056，gpt-6.1-sol/high（M侦察）；基线37fa6f0，无分支/构建。E02 v1 golden与E00 CI缺口、最小白名单/真源V映射与可离线验证范围；尚未实施。

#### 2026-10-01T11:44:12.096826+00:00 AG-054 派发前设计复核
- 草案cards/AG-054-design-r4.md SHA256 fa74b578596c92650ec6ece3c0a0765681a4e455283af66f9dcc8a76745b2ac8。SC-AG054-R2交原侦察 /root/scout_ag054，gpt-6.1-sol/high；只读核reserve错误类别及Released重放、白名单与测试，不实施。最终裁决待正式卡。

#### 2026-10-01T11:49:49.010424+00:00 CTRL-AG053-R1 / #99 独立验收通过
- 固定head a2c62326c7ca87a724da16ee4cfd133462b49dbe，父37fa6f08e9ef779b796b9713bc4664511321aa7e。交付f5214bca…叠放补丁逐字节一致，唯一lib.rs +280/-3。远端head一致、两树探针后干净。
- qa/ctrl-ag053-a2c6232.log*：全新target-ctrl-ag053，fmt/clippy/workspace/build/2smoke/checker/unittest全部退出0。workspace1130/0=main1124+新增6，81二进制+5doc组；unittest48。另定向storage lib21/0日志qa/ctrl-ag053-storage-lib-a2c6232.log。
- 主控反例qa/probe-ag053/append.rs SHA256 ba5039042d17e5fb8fa252fc43d20194bd81f8c41c1a550616cac15ac009f6b2；固定源source-local_export-37fa6f0.rs SHA25662e8afcacdd8146e79f24f7eba3ea917d9c68ecb298f1a0a769483df6e464f7c。head日志qa/ctrl-ag053-adversarial-head-a2c6232.log、父日志qa/ctrl-ag053-adversarial-base-37fa6f0.log，各2/0、退出0，源码树/target各自独立。
- P1同一多kind菱形/环边集的深层candidate正文变更，公开导出发布Conflict且无目录/audit/完成状态；P2同id跨namespace的正文/tombstone/edge不能重绑快照，错误namespace预检Forbidden，原scope成功且其他namespace仍撤销。两者基线同过，明确为公开消费侧语义保持边界，非性能缺陷复现。
- 资源回归：实施方基线4/2失败，长链747060→2787061；无关边4075→12804005；头6/0及主控全量复跑全部通过。R1夹具ANALYZE仅长链、2倍门不变；原未ANALYZE基线中断130保留，不记通过。所有实施材料qa/impl-ag053/。
- verified仅E08/E16.5，§3.3.1 L319/§11 L1081/E08 L1295,L1299/E16.5 L1451,L1453；V017/V069/V075导出依赖核验、V038受限规模拒绝子范围。不可声明总边扫描/队列绝对有界、N+1消除、完整闭包清理、生产容量/时延、Linux/真实效果/发行。返修0，测试机制裁决R1一次，无冲突；回滚37fa6f0。
- 首次PR栈位更新EOF，重试成功。下一步固定head ready/merge；合并前再fetch复核父等于main。

#### 2026-10-01T11:50:57.872402+00:00 AG-059-R1 白名单注释同步
- cards/AG-059-R1.md SHA25623ddc7a475437ad2e36d5dfbfc63942bd22281d8170968738a8d0d528ec92c71，依据§11.3 L1110/§11.5 L1132，允许verify_sources文档调用计数随本次新增闸同步，先核真实调用点；代码与seeds不改。原实施max档不变。

#### 2026-10-01T11:52:52.753187+00:00 合并 AG-053 / #99
- fixed head a2c62326c7ca87a724da16ee4cfd133462b49dbe；merged_sha 3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360；mergedAt 2026-10-01T11:51:03Z；accepted tree=merged tree=b5855ddf193f72b1b9dd43622e01d54b8c887c4d，一致。本地main已ff-only到origin/main。
- --admin未使用（CLEAN且无审批保护要求，固定head普通merge成功）；实现返修0、测试裁决R1一次，无冲突。清理随后执行。
- 过程偏离：Controller acceptance正文更新遇EOF，主控错误地继续ready/merge，随后立即重试补写并读取核对成功；实际独立全量/反例及本地CTRL记录都在merge前完成，验收head不变。此顺序偏离明确披露；后续把正文更新成功作为ready的显式前置条件。

#### 2026-10-01T11:56:05.848971+00:00 AG-053清理与AG-054派发
- #99工作树/父对照干净且均为main祖先，正常remove，本地branch -d，远端逐一删除；自身实施/验收/base targets删除，QA全保留。磁盘32GiB，main=origin/main=3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360干净。
- AG-054 E04/§7.2/§7.2.1、V008/V096/V086限卡中子范围。卡cards/AG-054.md SHA256d167cdaa3c34fc28ad07089d65f1764d3367f5e063c8708ce44b6eae4d99ad0e；正式裁决取代草案，版本化原因码防旧记录误解码，generic DB/未知/坏source不退款，旧storage API保持。
- 基线3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360；worktree out/work-20260930/ag-054；分支wrokbot/ag-054-typed-predispatch-refusals。实施 /root/impl_ag054，gpt-6.1-sol/max（L跨crate/计费/恢复安全缺陷）。与052/059文件不重叠；实施3，编译最多3。
- SC-AG055/056完成，只读事实保存于会话；后续正式卡可纯golden测试/CI。AG056纯workflow只能加CI固定迁移摘要门，不能声称smoke脚本自身覆盖0005/6；此范围须正式裁决。尚未实施。

#### 2026-10-01T11:58:35.612644+00:00 AG-052本地交付及主控接管发布请求
- 用户辅助chat自动审批拒绝其git push，理由未识别该chat中的直接用户授权；主控未让其绕过，要求冻结并完整报告。root有直接用户授权：用户指定启动提示词SHA256并要求逐条执行，§3明文draft PR到acosmi/RSIAgent及固定head推送；再次核对hash/范围/远端。
- 本地fc381036482727454f7b008a51fec99a8e2d581b，父37fa6f0；两文件+1432/-15（streaming_evaluator.rs +122/-15，新streaming_verdicts_v42.rs +1310），已全部读diff与测试。无out/方案/归档/秘密，root已用require_escalated显式说明原拒绝原因及可信授权，提交限定push请求。
- 实施结果：基线3/7失败，新integration10/0+private2/0；workspace最终1136/0（旧main1124+12），沙箱HTTP5个失败已保留并提权全量复跑。主控叠放后应1130+12=1142；尚未主控验收，不合并。

#### 2026-10-01T12:00:15.745134+00:00 AG-052 / #100 叠放与独立全量启动
- 主控在直接用户授权下require_escalated发布已通过自动审批，push成功；draft PR #100 https://github.com/acosmi/RSIAgent/pull/100 已创建附到chat，辅助窗口阻断不再存在于主控请求。只提交两个审阅过的文件。
- rebase从37fa6f08e9ef779b796b9713bc4664511321aa7e到3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360，fc381036482727454f7b008a51fec99a8e2d581b→652d7cac0d6cf1fa086e96b6ac9554ed6326b1da。单提交patch cmp0（qa/ctrl-ag052-before-rebase.patch、after），无冲突；literal full refs/head force-with-lease推送。
- 全新target-ctrl-ag052，qa/ctrl-ag052-652d7ca.log*正在独立全量，预期1142=1130+12；worktree复跑期间禁止添加文件。原实施日志复制qa/impl-ag052，最终report到达后补归档。
- AG059最终tests-only基线17/4失败，日志baseline-final-tests，四失败为目标缺陷；初版fixture容量/namespace/shared未知cleanup错误已披露，仅修新夹具，不改产品边界。既有import→optimization.source清理unknown_scope不在本任务实施范围，后续归入typed边完整性/实际支持记录清单复核。

#### 2026-10-01T12:04:56.807900+00:00 AG-052验收进展与SC-AG058派发
- #100独立全量qa/ctrl-ag052-652d7ca.log*全部8项退出0；workspace1142/0（1130+12），82二进制+5doc组，unittest48。head两个主控探针2/0且clean，父树对照启动。
- AG-052最终report已归档qa/impl-ag052/report.md，含辅助chat两次自动审批拒绝原文。root重新核对直接用户指令、payload和目标后自动审批允许，#100成功创建；不是借助辅助报告授予权限。child交付已结束，后续只读。
- SC-AG058-r4卡SHA2563fba98dbd8b0e1a93dbf963f6be9141547b94665f44667dddef27feb9d41b3e4，派用户授权chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/medium（S），基线3aa3804；AG058过期文字的真源/消费者事实，禁止任何写入/构建/发布。无实施分支。
- AG059披露初读_common前AGENTS文件名搜索范围过宽（发现其他参考仓路径但未开内容/进入/修改），之后严格本WT；已记过程偏离，未授权扩展范围。

#### 2026-10-01T12:05:36.364405+00:00 SC-AG057只读派发
- /root/scout_ag057，gpt-6.1-sol/high（M API兼容），基线3aa3804；E01文档/显式样本量及旧冻结计划语义，核E/V与最小接口，不实施、不构建/写文件。

#### 2026-10-01T12:06:46.429786+00:00 CTRL-AG052-R1 / #100 独立验收通过
- 固定head652d7cac0d6cf1fa086e96b6ac9554ed6326b1da，父3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360；交付fc381036…两文件+1432/-15逐行已读，单提交rebase patch逐字节相同，无冲突/产品返修。
- 全新target-ctrl-ag052，qa/ctrl-ag052-652d7ca.log*，全部8检查退出0；workspace1142/0（main1130+10集成+2私有单测），82二进制+5doc组；unittest48。check结构/计划绑定成功。
- 主控源probe-ag052/source-streaming-v1-37fa6f0.rs SHA25636d3bdb5b77946ab6715624eaefc6cf5b0f2a45ed67c65a6e9fafda9e32cc37b：固定原streaming_evaluator测试拷贝，只将控制schema常量改成v1字面量，确保两树相同且测旧control。append.rs SHA2560a370309dccf9c761791339262ef51e5fd541531dafcbb268ca007b9c2fe20cf。
- head日志qa/ctrl-ag052-adversarial-head-652d7ca.log 2/0退出0；父树qa/ctrl-ag052-adversarial-base-3aa3804.log 0/2退出101，均正常编译且事后clean。P1合法v1控制只改approver_actor、schema/统计计划不改；P2只把ticket.registration_digest换为不同合法digest；旧版二者都Ok(CompleteBatch)，新head均Conflict、两次重复无artifact/audit写入。独立于实施方只改schema的篡改用例。
- verified E05，E05L1253/L1257/L1259、§3.4L337、§3.4.1L372、§6.6L709/L711；V010/V012/V084/V085仅本地程序完整批次、冻结控制版本/摘要/恢复/晋级拒绝范围。v2保留Improved/Invalid/原Inconclusive，negative_lcb不称确认退化，成本节省无统计证明仍压Inconclusive；v1原字节/reasons保留。晋级门恒拒。
- 不可声明新固定样本UCB算法、core decide已修、真实效果/生产验收权/可晋级/完整成本与尾延迟统计保证、所有新请求自动v2、全部E/V或Linux。提交事务二次读取控制检查代码已复核，无额外暂停hook模拟任意调度；不能夸称并发穷举。回滚需独立revert保留历史/费用，旧程序对v2失败关闭；父提交3aa3804仅源码回滚参考。
- 辅助chat两次auto-review因无该chat可信用户授权拒绝push，报告原文在qa/impl-ag052/report.md；主控核本chat直接用户指令与实际payload/remote后正常通过审批并建PR。已披露交付适配，不隐藏拒绝。

#### 2026-10-01T12:10:22.209696+00:00 合并 AG-052 / #100
- fixed head 652d7cac0d6cf1fa086e96b6ac9554ed6326b1da，merged_sha43f0022478d58dd8a8e6492cffb3aa24463b3fce，mergedAt2026-10-01T12:08:10Z；双方tree=430461d69007524b1f531ed3ec457f722f86dcff。merge的第一父提交确为3aa3804db4ec39a72f38a8ec00cc7e6f9e92b360，第二父为验收head。主控树核对后ff-only更新main，未用--admin（CLEAN/无审批要求）。
- 本次验收正文写入及逐字读回先成功，再ready/merge。另有过程偏离：合并前fetch遇TLS失败，functions.exec中后续工具调用未依退出码停止而继续merge；原origin/main值仍旧值。合并后fetch重试成功，确认实际父确为已验父、tree完全一致；没有合入未验树，但合并前刷新闸执行顺序不合要求。后续所有合并将采用单个set -euo pipefail脚本，fetch/正文读回/head/base/clean任一失败立即退出，不再跨多个工具调用假定成功。
- 实施返修0，无叠放冲突；辅助chat发布阻断已由root基于直接用户授权合规接管并通过审批。二次清单已更新，清理随后执行。

#### 2026-10-01T12:19:34.558209+00:00 压缩后启动闸门、AG-055派发与AG-059交付工具裁决
- 提示词b0000207…、真源70ec06e4…一致，完整重读启动提示词与规定状态/队列/卡/探针/台账；gh auth/fetch、cargo1.98.1、Python3.12.14通过，磁盘33GiB。main=origin/main=43f0022478d58dd8a8e6492cffb3aa24463b3fce干净。
- #100清理已完成：干净且已合入的AG052/父对照工作树正常移除，本地branch -d、远端逐个删除，实施/验收/base target移除；raw QA全部保留。
- AG-055 E02/V002旧v1 wire/四工具/未知字段子范围；卡cards/AG-055.md SHA256853308a8c4f301df3cde2cc8e3a3c3382d9ba2ae6ac0901ef2dfe1f89dac7060；依据§5.1 L453/L455、§6.1 L605/L609、E02 L1213/L1215、V002 L1502、L2166。裁决仅新增golden测试，旧Evaluation/float/typed字节解释保持，不声称RFC8785或产品缺陷修复。
- 基线43f0022478d58dd8a8e6492cffb3aa24463b3fce；worktree out/work-20260930/ag-055；branch wrokbot/ag-055-v1-contract-goldens；实施/root/impl_ag055，gpt-6.1-sol/high（M）。与054/059无重叠；实施3，059构建结束，root验收+054+055最多3路。
- AG059交付工具裁决：gh GraphQL create两次EOF、REST查询确认无PR；允许等价REST POST创建同head/base/main/title/body且draft=true，任何未知结果先查询，禁止重复PR。依据启动§3.4同一draft交付要求，仅工具回退，不改变卡/文件/产品行为。

#### 2026-10-01T12:23:07.675616+00:00 AG-059 / #101交付与叠放
- draft PR https://github.com/acosmi/RSIAgent/pull/101，交付4c403ebc68c8c66bfd2c45f74778f4cef884f189；REST同等draft创建成功，GraphQL EOF原始日志保留。两文件+693/-58逐行已读，产品只有联合all_sources首写前verify_sources及R1调用数注释；测试8项新增且仅授权旧断言改变。
- 原基线37fa6f08e9ef779b796b9713bc4664511321aa7e叠放到43f0022478d58dd8a8e6492cffb3aa24463b3fce，新head8257e6a73304c250b545ead3c0bc9409cf2daa35；单提交前后patch逐字节cmp0，无冲突。首次fetch TLS失败脚本立即退出未变更，重试成功再rebase，固定旧head lease推送成功（首推TLS后重试）。
- 实施证据已归档qa/impl-ag059：baseline最终17/4失败；定向55/0；workspace首次沙箱5HTTP PermissionDenied，提权最终1132/0=旧main1124+8，其他全部0。主控全新target-ctrl-ag059运行中，日志qa/ctrl-ag059-8257e6a.log*，应1142+8=1150。未知cleanup范围和storage-kind边界继续保持。
- 归档脚本首次调用提权环境默认python3遇编码错误、未归档，已改绝对out/pybin/python3重跑；验收脚本自身指定PATH不受影响。

#### 2026-10-01T12:24:54.930302+00:00 SC-AG062派发与AG054-R1
- SC-AG062卡SHA256d3d4f9206cb5d08e44928a2d2b95f3ccc92e30436386559f02980f87998c536f，用户授权辅助chat01a0f723-2cc0-7b10-943b-f03e5c6f95cc，gpt-6.1-sol/high，基线43f0022只读E02 FieldContract/P-B绑定，尚未实施。SC058报告已存cards/SC-AG058-r4-report.md。
- AG054-R1 ae9c3c1a5d7eba66ec755ebfcd35f50cfd366cd24d2837947f2bf927cf80c320：依据§7.2 L819/§7.2.1 L831/E04 L1239,L1243，保留空sources恢复的原成功语义，两类API均不删除持久来源；source_closure_changed限原非空比较分支，不新增typed-only强制完整来源列表。须测空恢复后来源/金额不变、begin仍按持久源撤销拒绝。max档/白名单不变，卡已发实施。

#### 2026-10-01T12:29:34.898268+00:00 CTRL-AG059-R1 / #101独立验收通过
- 固定head8257e6a73304c250b545ead3c0bc9409cf2daa35，父43f0022478d58dd8a8e6492cffb3aa24463b3fce；原交付4c403eb…重放一提交patch逐字节cmp0。两文件+693/-58逐行复核，产品+4/-1只联合首写前核验及R1注释；测试新增8项。
- qa/ctrl-ag059-8257e6a.log*全新target，8项退出0；1150/0=1142+8、82二进制+5doc组，unittest48。
- 主控probe-ag059/source-write-side-3aa3804.rs SHA256e2f9b6ff8235ba5da2fe4edb643cd19f526a4d4501bb2336a08055dd5feced4d，append.rs SHA2561bf7fb8c4c441a7dde7046b1b383c7626c8f1d1145e9ebc4294645257b2683b8；head日志ctrl-ag059-adversarial-head-8257e6a.log 2/0 exit0，父ctrl-ag059-adversarial-base-43f0022.log 0/2 exit101。P1缺失+已解析脱敏依赖混合，两序/重复；P2同publisher/asset而kind不同的skill/improver，live skill正向与revoked improver混合，旧树均写Prepared/audit/边/blob后拒，新树无变更。两树clean。
- verified E08/E16.2，§11.3L1110/§11.5L1132/E08L1295,L1299，V017/V075已解析manifest与请求联合首写前拒绝，V038现有联合10000界；missing保持quarantine。不可声明storage-kind/物理擦除/未知清理schema/真实宿主/吞吐时延/Linux/整体E-V/收益/发行。回滚参考父43f0022，不删除撤销/账单事实。
- 接受实施夹具纠正（容量、namespace、unknown cleanup）且原失败日志保留；最终tests-only基线17/4失败。产品返修0，R1注释授权1，无冲突。GraphQL→REST交付回退与首次只读AGENTS文件名范围偏离均披露。验收正文已生成，必须写入读回成功再ready/固定head merge。

#### 2026-10-01T12:32:11.224591+00:00 合并AG-059 / #101
- fixed head 8257e6a73304c250b545ead3c0bc9409cf2daa35，merged_sha 296b5a07a7bf911d02454b683d5732e9d38e3ae5，mergedAt2026-10-01T12:31:01Z；两树均87dc2da1cb72494b7241792c96b4eb2341009a1f，merge父为43f0022478d58dd8a8e6492cffb3aa24463b3fce+验收head。main已ff-only。
- 采用qa/merge-ag059-8257e6a.sh单一set -euo pipefail脚本，验收正文REST写入并逐字读回、remote head/base/draft/clean、fetch当前父、ready后再次head/CLEAN及fetch全部成功后才固定head合并。日志qa/ctrl-ag059-merge.log。admin=false，未有保护审批要求，无冲突，产品返修0、R1注释裁决1。
- 原实施最终报告已归qa/impl-ag059/handoff.md；实施的最终head后置检查exit1是主控接管rebase导致，已核对非源码意外变化。二次清单即时更新；清理随后执行。AG060后已合并3产品任务，现触发AG061台账PR。

#### 2026-10-01T12:33:29.256198+00:00 AG054-R2与AG055发布审批边界
- AG054-R2 cards/AG-054-R2.md SHA25630db20fa59bf3217fb13245286df618550df58dc60145bf0412f225d58537e69：补授权engine budget_revoke_gate_v42两处既有断言，来源撤销broker Unauthorized固定码、dev Released且0预留；依据§7.2L819/§7.2.1L831/E04L1239,L1243，其余未知费用/无dispatch/receipt不变。
- AG055自动审批拒绝其commit/push/create组合，认为未识别该payload到目标repo的直接用户授权；子代理未执行，正完成本地交付。root将独立审阅17新增测试/fixture及目标，引用本chat直接用户哈希指定提示词§3.4/§10授权，连同原拒绝提交审批；未获准不外发、不用子代理报告代替权限。

#### 2026-10-01T12:35:11.534371+00:00 AG059清理及AG056派发
- #101两工作树干净、heads均origin/main祖先后正常remove；本地branch -d、远端单分支删除；实施/验收/base target清理完，QA全保留，磁盘27GiB。
- AG056 E00 V001/V071/V072/V073/V080/V098限定CI定义；卡cards/AG-056.md SHA256a996d6f23750c37f2d4af6b55a6fa0521c7f7467714ce11b878d144987ddb14d（草稿d92dc2ac…派发前更新基线/编译许可）。基线296b5a07a7bf911d02454b683d5732e9d38e3ae5；worktree out/work-20260930/ag-056；branch wrokbot/ag-056-ci-quality-gates；/root/impl_ag056 gpt-6.1-sol/high（S）。仅ci.yml，不派Actions；0005/6为独立CIchecksum门，非smoke脚本扩充。
- AG054+055+056实施至多3；055已结束构建，root台账验收+054+056最多3编译。AG061由主控准备，3个产品合并触发，不等用户确认。

#### 2026-10-01T12:35:40.251841+00:00 AG061主控台账任务启动
- 卡cards/AG-061.md SHA256beb3c160ec5d99105aadec68bb4f567f8edfe3b2c32ae368975dea4d496eb9ab；E00/E16.6 V001/V071/V072/V073/V074/V080/V098限定证据登记；主控Codex自做，不虚构实际模型。
- 基线296b5a07a7bf911d02454b683d5732e9d38e3ae5，worktree out/work-20260930/ag-061，branch wrokbot/ag-061-controller-ledger-r4a。折入r4至本事件，补#99–101三条子范围，刷新1150基线/队列；原始材料仅本地。

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
| E00 | 归并真实源码与可重建输入 | implemented_not_verified（索引同口径；核心固定基线子范围 verified，历史归并 blocked） | 原始112测试与门禁均复验通过；脚本已完成10项主控正负验证；源码或fixture变更后旧日志不能用于新输入；历史缺包不隐藏。2026-09-30/10-01：`smoke_cli.py` 过时使 CI 门失败，AG-013 已修并合并（#59，merged_sha `69a3b3ed04377c30ee7193c0679a3747f1606a1e`）；派生索引/检查器绑定 v4.2（AG-012，#60，merged_sha `dc036d5f4dd5d870fb0ca98bdfe1ef6ec626140b`）；裸 `--data` 默认路径无法加锁启动已修（AG-023，#68，merged_sha `3ffe1e26a908d4c9240320d52fcba1e92dc383d4`）。历史包与 T001–T126 归并仍为外部阻塞。  当前基线212c784，独立全量1273通过/0失败（91测试二进制+5doc组）；AG056本地完成但OAuth缺workflow scope，未发布/验收；历史基线证据保留。|
| E01 | 先冻结实验、任务分区和预算可行性 | blocked（索引同口径；静态合同子范围已 verified） | 本地bfbed84；显式n/统计前提、alpha/留出/比较/完整费用合同已验；开发小试/正式样本与付费授权仍缺。  AG-057/#106已验显式样本量构造器、登记文档与旧v1字节；Rust API需迁移为(id,n)，2..100000不证明功效，真实任务/oracle/锚点与GC22仍缺。 |
| E02 | 最小版本化契约与宿主能力边界 | in_progress | 纯编译作用域：有界原子编辑；运行/宿主/其他新增契约尚待后续消费者。  AG-055/#103已验五请求/四真实描述符及旧Strategy/Evaluation golden；FieldContract、P/B真实绑定和新宿主契约未完成，AG062/#108非空P/B标签门与AG065/#110名称反向覆盖已验；类型/默认值/真实提取与消费者、已完成run冻结复用仍不在声明内。 |
| E03 | 把跨任务证据真正接入生成消费者 | in_progress（消费者/恢复已verified；开发回执子范围已verified并合并） | 主控144项core/engine测试、fmt/clippy通过；实际ModelPort请求、同清单开发选择及持久恢复已验。2026-09-30 AG-016（#63，merged_sha `3e2fc067190077b8cf8401875d6c627b8d9e13fc`）：DevelopmentControl/执行回执/grader 回执 typed schema、已登记纯函数运行器与真实观察门禁。E03 门验证的是回执链自洽，不复算已登记纯函数的输出，也未把执行方身份绑定到登记的执行器（AG-033 验收观察）。真实提供商/样本/收益、隔离运行器未验。  SC-E03复核发现Host登记诊断尚未核实际应用绑定；真实规则定位/行为/父版本与事件引用合同及该消费者缺口仍未关闭。 |
| E04 | 可信执行、隔离与根资源预算 | in_progress（根预算/broker已verified） | 主控预算15（含10001调用）、broker9、executor6项及fmt/clippy通过；group完整分页停止修复已追加PR #33；真实提供商、进程隔离尚未验。  AG-054/#104已验typed确定拒绝、仅Reserved且完整fence清理及unknown不退款；GA14阶段上限仍只静态校验，根计划绑定/版本/阶段映射合同待明确。 |
| E05 | 独立验收器与有边界的统计判定 | blocked（索引同口径；持久控制/早停子范围已 verified） | 主控12流式+6旧评测+13core共31项及fmt/clippy通过；晚到回执、输出不可变、Exposure时间/终态/未知费用保留已修；AG-052/#100已验显式v2完整批次结论与完整登记绑定，v1旧行为保留，非UCB确认/成本未证不扩声明；真实提供商、进程隔离、完整成本及逐依赖撤销仍未验，fixture禁止晋级。 |
| E06 | 组合发布、实际应用与最小撤销闭环 | in_progress（持久门禁已verified） | 主控7集成+4单元+12 E05回归及fmt/clippy通过；审批防回退、Host完整报告闭包、所有快照读入口已验；真实生产批准/组合应用/回滚受E05证据阻塞。 |
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
