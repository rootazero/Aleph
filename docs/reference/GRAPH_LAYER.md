# GRAPH_LAYER.md — 循环治理图 (Loop-Graph Governance)

> Spec 母本：[docs/superpowers/specs/2026-07-19-graph-engineering-loop-graph-layer-design.md](../superpowers/specs/2026-07-19-graph-engineering-loop-graph-layer-design.md)（含 11-agent Workflow 评审记录与全部理据）。本文是实现后的运行参考。
>
> 一句话：**代码只持有"谁看守谁"的拓扑与不可辩驳的事实；一切裁决是普通会话里的一次 LLM 推理；根参照由人从图外供给且机器在通道上不可达。**

## 1. 动机（30 秒版）

单一自改进循环有四种结构性失败——Goodhart（指标被优化到脱离本意）、参照盲区（环无法质疑自己的目标）、循环冲突（独立建的环互相打架）、测量衰减（没人看守看守者）。Aleph 的循环很多（goal / cron / heartbeat / dreaming / context 压缩…），四种失败全部有过实锤（做梦 8 天静默烧光 Kimi 月配额＝测量衰减旗舰事故；budget.rs 手算口径漏计 846 行＝核对文书而非现实）。解法是拓扑的：**配对、层级、仲裁、审计环**，外加图之外的**锚点/冻结节点/人供根参照**——无锚的图会环形失败（一切一致、无一被验证）。

### 两种 "Graph Engineering" 辨析（防撞名）

| | 本层（改进环之图） | KG 图数据工程 | LangGraph 式 agent graph |
|---|---|---|---|
| 节点 | 完整的改进循环 + 锚点/冻结/根参照 | 知识实体/笔记 | 单次执行内的工具/LLM 调用 |
| 边 | watches/owns_reference/audits 等治理关系 | 语义关系（自由词表） | 控制流转移 |
| Aleph 对应 | **本层 `src/loop_graph/`** | `src/memory/notes/`（已有） | **不建**（违 R10 笨循环） |

两域互补：第一域给拓扑设计，第二域给存储/维护手艺；交汇点是 dreaming（既是 KG 维护管线，又是本图的头号被审优化环）。

## 2. 融合裁决（与记忆图）

**结构不融合，结论融合，算法与画布只读复用**：

- **拓扑不进 notes 库**：notes 是 dreaming 每晚改写的可写域，把"看守优化器的拓扑"存进被看守者可写的库＝违反冻结原则（RewardHackingAgents 实证 ~50% episode 自然尝试篡改评估器）。自持 `~/.aleph/data/loop_graph.db`。**dreaming 及一切优化环对 loop_graph.db 永无写权限——这是本层自身的冻结声明。**
- **裁决进记忆**：审计/仲裁判决书由 LLM 用 `note_manage` 写（category=lesson，tags 含 `graph-audit`，YAML 证据块：audited_node/evidence_cmd/evidence_result/evidence_ts/verdict∈{pass,drift,cheat,stale}）；参照变更提案 tag `reference-proposal`。
- **只读复用（Phase 5，defer）**：`src/memory/notes/graph/` 纯算法层（社区/洞察）与 canvas 星系投影（`loopgraph.*` RPC 命名空间，勿并入 `graph.*`）。

## 3. 代码地图

| 组件 | 位置 | 说明 |
|---|---|---|
| 类型（6 词闭集边） | `src/loop_graph/types.rs` | `NodeKind`(loop_goal/loop_cron/loop_heartbeat/daemon/**team**/anchor/frozen/root)、`EdgeKind`(watches/owns_reference/arbitrates/audits/anchored_by/feeds)、`Origin`(human/llm，provenance 一等) |
| 拓扑事件总线 | `src/loop_graph/events.rs`（2026-08-14 新增） | `TopologyEventBus`（`tokio::sync::broadcast`，cap 256）：`NodeUpserted`/`NodeDeleted`/`EdgeUpserted`/`EdgeDeleted`/`GcCompleted`。进程级 `OnceCell`（`init_event_bus`/`event_bus`/`publish`），bus 缺席时发布为 no-op；工具的写面（node/drop_node/link/unlink/gc/enable_audit/pair）在每次成功变更后发布。**不是 WAL**（快照才是）、**不是 hook 系统**、**不是写屏障**——store 不变量先于 publish 执行 |
| 时间快照 | `src/loop_graph/snapshot.rs`（2026-08-14 新增） | `SnapshotStore`（独立 `loop_graph_snapshots.db`，JSON blob 存节点/边全集）：`capture`/`list_snapshots`/`get_snapshot`/`diff_snapshots`（BTreeMap 按稳定键 diff，`Added`↔`Removed` 对称、确定性排序）/`delete_snapshot`。**刻意无 restore**：快照是审计记录不是回滚原语（GRAPH_LAYER §7 NOT-build「time-travel/fork」的精神在此成立——角色图的「可审计」= 可回放历史，不是可分叉现在）。`capture` 用 `last_insert_rowid`（`execute()` 返回的是受影响行数） |
| 只读查询门面 | `src/loop_graph/inspector.rs`（2026-08-14 新增） | `LoopGraphInspector`：`subgraph_for`（ancestor/descendant 沿 `owns_reference` 双向走，1024 步硬顶防爆；coverage_sources 用与 lint 同一 `can_cover` 谓词过滤——anchor/frozen 不算看守）、`impact_of_removing`（**纯模拟**：post-state 保留悬空边——真实 `drop_node` 不级联，模拟也不该假装它级联）、`summary()`、`loops_with_coverage()`。统一四个读者此前各自手搓的读路径（`render_session_topology`/`notify_goal_settled`/`governing_owner`/`status` live-join） |
| 拓扑导出 | `src/loop_graph/export.rs`（2026-08-14 新增） | `to_dot`（Graphviz：按 kind 分形状/颜色，body 只进 tooltip 截断 200 字符——root body 可能很大）与 `to_json`（Panel/审计日志用）；两者**确定性字节**（kind/id、from/to/kind 排序），缓存安全。零新依赖（DOT 内联模板，不引 petgraph） |
| 存储 | `src/loop_graph/store.rs` | 两表（graph_nodes/graph_edges，agent_id 作用域，PK 复合）；读写两侧都走 `utils::sqlite_open`（写 `open_sqlite_safe` / 读 `open_sqlite_readonly`，后者的 `busy_timeout` 让 doctor 不会把并发写误报成「Graph DB unreadable」）；**三条 store 级不变量**：root origin=human、**治理边禁自环**（`watches`/`audits`/`owns_reference` 的 `from_id != to_id`——否则一次 `link` 就能让 `lint_naked_loops` 对该环永久静默，即本层要检测的测量衰减本身）、**`origin` 写一次不再被 upsert 改写**（provenance 是审计模板要查的东西）；**`note` 与 `body`/`cadence` 同为 `COALESCE`**（省略即保留——re-`link` 一条已有边不得把人写的理据 NULL 掉，清空传 `""`）；无 FK 级联（悬空边=审计信号，显式 `gc` 才清——**但 `owns_reference` 永不被 `gc` 收走**，2026-08-03 round 11：它不是装饰，是 `governing_owner` 读来拒绝被治理环改写自己 objective 的那一行。治理者节点消失后这条边**悬空但仍在生效**才是对的——「我的治理者不见了」不能是一次自助解锁。曾经 `drop_node`＋`gc` 两次调用**都不带受保护 id**，因而两次都不举卡，合起来就是 §6.2 写保护的第二扇门，与 `unlink` 那扇并排。`GcReport{removed, retained_acl}` 必须把保留说出来——被要求清理却静默留着，和静默删掉是同一类谎）；**`body`/`cadence` 省略即保留（`COALESCE`）**——工具侧两者都是 `#[serde(default)] Option<String>`，全量覆盖会让「改个 label 重登记一次 `root:`」把人写的根参照原文写成 NULL，而两个读者都只在 `Some(body)` 时渲染 root 行，于是那一行从此后每一个被治理会话的 prompt 里静默消失（要清空传 `""`）；**存在性只问 `node_ids_present`（直读 `id` 列）**——`row_to_node` 对未知 enum 文本 fail-soft 跳过是给读者防卡死的，拿它当 `gc` 的 DELETE 判据会把「读不懂」当成「不存在」，不可逆删掉一个仍然存在的节点的所有边（两个 enum 都 `#[non_exhaustive]`，降级运行就是触发条件），`lint_dangling_edges` 共用同一份；`lint()` 纯结构检查（悬空/裸奔优化环/**伪造的看守覆盖**/治理链未锚定 root（**全路径 BFS**，多 owner 时只要有一条路汇于 root 即算锚定）/快环拥有慢环参照）。**`lint_forged_coverage`（2026-08-03）**：round 9 让豁免在长度 1 上不可伪造（`from_id != n.id`，写入期也拒），长度 2 仍免费——两次 `link` 就让优化环与它的审计员互看、双双静默，正是本层要检测的测量衰减本身。判据是**向上走 `watches`/`audits` 是否绕回自己且环上没有任何 `Root`**。刻意窄：**只打环，不打「尚未锚定的线性链」**——后者的顶点已经作为裸奔环报了一次，逐节点复述同一个根因只会淹掉真发现。**round 11 收敛了它的接地集合**：见下条。<br>**覆盖边的来源必须能跑（`coverage_source_rejection` / `can_cover`，2026-08-03 round 11）**——round 9 管住长度 1、round 10 管住长度 2，两次修的都是覆盖环的**形状**，而**词汇**一直是开的：`watches`/`audits` 的**来源**可以是任何 kind。`anchor:` 节点只要一次**不过审批卡**的 `node` 调用（卡只匹配 `root:`/`frozen:`），于是两次免费调用就同时满足 `lint_naked_loops` 并给 `lint_forged_coverage` 接地——整层唯一的自证豁免检测器就此永久静默。判据是「看守必须**跑得起来**」：`Anchor` 是一次测量声明、`Frozen` 是别处执法的规则，两者都没有会话、没有节奏、复核不了任何东西；`Root` 保留（＝**人**读 digest，三层封顶的第三层，且 root 写受 store 级 origin=human + 审批卡双重约束）。写入期 `upsert_edge` 拒、两个 lint 对**存量行**同样折价（不依赖迁移）。副作用：`lint_forged_coverage` 的接地集合按构造塌缩成 `Root` 一种（向上走只会经过来源位置），代码与文档同步简化 |
| 模板（智慧在此，R9） | `src/loop_graph/templates.rs` | `AUDIT_TEMPLATE`（七步审计）/`WATCH_TEMPLATE_HEADER`+`_FOOTER`（看守）。**仅此两类**——原 `STEWARD_TEMPLATE`/`ARBITRATION_TEMPLATE` 零消费者已 CUT（2026-07-24，R10 YAGNI）：steward/arbitration 的教义活在 `loop-governance` skill，此类环按需用 `cron_manage` 手建（仲裁刻意是事件非常驻服务，勿建安装器）。⚠️ **模板点名的每个工具都要问「谁来调它」**（2026-08-03 round 11）：AUDIT_TEMPLATE 的取证步默认派 `subagent(agent_type="loop-auditor")`，而审计员的白名单**刻意没有** `cron_manage`（多路复用工具含写动作）——所以常备锚点 ② 的 cron 运行计数**只能由审计 cron 会话自己取**，模板现在直说这一点（否则那是一条必然失败的委派）。同批让模板顺手核对名册里有没有**两个** `循环治理·审计环`（重装留下的孤儿，见工具行 round 11 ①）|
| 触发与会话服务 | `src/loop_graph/service.rs` | `notify_goal_settled`（胜利宣称时刻戳看守 cron，60s 去抖；**返回「有没有真 poke」**——一次性章必须等 poke 确认后才算花掉，否则「cron handle 没挂 / `run_job` 报错 / 看守不是 `cron:`」三条出路都会让这次完成**永远**等不到评审，因为 Complete goal 的 `completed_at_ms` 此后不再变）、`watcher_is_pokeable`（**只有 `cron:` 看守能被即时唤醒**——poke 就是 `CronService::run_job`；手接的 `heartbeat:`/`daemon:` 看守照样满足 `lint_naked_loops`、照样渲染成「有人看着」，图看起来健全而评审要等它自己的节奏，故 `link` 在写入那一刻就说明，唯一还来得及纠正的时刻）、**`target_has_victory_claim`（2026-08-03 round 11 补的镜像面）**——前者问「这个**看守**叫得醒吗」，后者问「这个**被看守者**会不会宣称胜利」：只有 `goal:`（`notify_goal_settled`，三个调用点走 store CAS）与 `team:`（`notify_team_settled`）有终态时刻可挂，`cron:`/`daemon:`/`heartbeat:` 根本没有，所以配到 `daemon:dreaming` 上的看守**永远**只按自己节奏跑。`pair` 的成功消息与 prompt 渲染两处都必须**同时问两个问题**（合取式 `immediate_review_reaches`）才敢承诺即时评审——一个方向被想到、反方向没有，是本仓这类缺陷最常见的形状；`governing_owner`（objective ACL 查询，**返回 `Result`**——读不到图必须拒绝写入而不是放行：把 store 错误折成 `None` 等于一次 `SQLITE_BUSY` 就关掉 §6.2 写保护，无错误无日志）、`render_session_topology`（prompt 注入渲染，**确定性字节**；⚠️ **`<loop_graph_context>` 是行式格式，行本身携带权威**——`escape_xml` 在 layer 缝上只封住 XML 那一层，`\n` 原样穿过，于是一个**模型自己写的 label**（`node` 对 `cron:` 前缀不举卡）就能另起一行伪造 `根参照 …（人供给——你可以引用、必须遵循、无权修改）: …`，逐轮注入每个被治理会话且持久。内层格式是行，就得连行这一层一起封：`one_line` 压平所有插值的 id/label/cadence，`indented_body` 把人写的多行 root body 缩进，使任何续行都当不成新的顶层语句。**外层格式的元字符转义 ≠ 内层格式安全**）、`watcher_jobs_for`（**返回 `Result` 且读原始列**——把 `SQLITE_BUSY` 折成「没有看守」，而调用方把「没有看守」读作「这次一次性章是白拿的，留着」，于是一次瞬时错误**永久**退休那次完成的评审：Complete goal 的 `completed_at_ms` 此后不再变。round 9 在 `gc` 和 objective ACL 上讲过同一课，这条路上漏了，而它是代价最大的一条）、去抖条目**带上它是为哪个节点取的**（`link` 是一等动词，一个看守可以合法覆盖多个环；把 B 的一次性章记在为 A 发起的那次 run 上——那次 run 早于 B 的胜利存在——就是拿别人的评审销掉自己的章） |
| 工具（R8 面） | `src/builtin_tools/loop_graph_manage.rs` | `loop_graph`(action: node/drop_node/link/unlink/list/status/gc/enable_audit/pair + **impact/export/snapshot（2026-08-14 新增，三者全只读除 snapshot capture）**)；`impact`＝`drop_node` 起飞前的爆炸半径预览（哪些环会失去看守、哪些 owns_reference ACL 失去治理者、post-state lint 预览——**纯模拟不落笔**，消息明说「纯模拟」）；`export`（format=dot\|json）；`snapshot`（op=capture\|list\|diff，diff 用 from_id/to_id 传快照数字 id）。anchor 强制 body 声明 truth∈{exit_code,numeric,line_count}；status 做 live join（goal store/cron jobs 实时状态，永不缓存观测）；**无 `agent_id` 参数**——图恒作用于 `routing::DEFAULT_AGENT_ID`（旧的模型可传旋钮是只写的：`service.rs` 与 doctor 的每个读路径都硬编码 "main"，非 main 作用域的图零 watcher poke / 零 ACL / 零 prompt 注入，而 `pair` 仍承诺 poke，2026-08-01 撤回）；`enable_audit` 的「已存在」闸判的是**活的审计环**——判据是节点 body == `templates::AUDIT_NODE_BODY`（与写入点同一常量），**不是**「存在某条 `audits` 边」：`Audits` 是一等动词（审计环→任意节点），手接一条 `cron:ratchet -[audits]-> frozen:budget` 是被鼓励的用法，用边当判据会让装过这种边的图**永远装不上审计环**，而报错还教用户去 `drop_node` 一个无关节点（照做就毁掉自己的治理边）；同理照它自己的提示 `drop_node` 之后留下的悬空边也不该挡住重装。**round 11 三条（2026-08-03）**：① **`enable_audit` 的幂等对齐现实，不只对齐图**——它自己印的重装建议（`drop_node` 再 `gc`）**从不删 cron job**，于是照做就留下一个仍按周跑、而 `status`/`list`/`lint`/doctor 全看不见的审计环，正是本文件里两处 rollback 注释写明要防的「两个审计环互相 supersede 裁决」；现在先扫 cron 名册找 `prompt == AUDIT_TEMPLATE` 的孤儿并**重新认领**（而不是再装一个），名册读不到就**拒绝安装**（这条路上「我没查到」绝不能当「没有」），`drop_node` 对任何 `cron:` 节点补一句「job 仍在跑，要停用 cron_manage」；幂等判据改读 `node_ids_with_body`（原始列，`list_nodes` 的 fail-soft 会把读不懂的审计节点读成「没有审计环」）。② **`audits` 扇出的逐边失败只记不抛**——跑到那里时 cron job 与标记节点都已提交，返回 `Err` 等于**对着一个活着的审计环报告彻底失败**，而调用方唯一的下一步（重装）会撞上幂等闸；边本来就是审计模板会自查的接线。③ **`node` 回报 store 里现在的 `origin`，不是传入的那个**——provenance 写一次不改写（这是对的），撒谎的是那句消息。<br>`status` 的 live join **区分「名册读了但没这条」与「名册读不到」**——一次 `list_jobs` 失败曾让每个 cron 节点都被打上「⚠ target missing」，而审计模板正是拿这行进点名步骤，一次瞬时错误就能对着一图健康的环制造审计发现 |
| 胜利宣称触发点 | `src/gateway/execution_engine/goal_continuation.rs` + `src/builtin_tools/goal.rs` | **三处**调 `notify_goal_settled`：continuation hook 的 gateless-terminal-complete 与 gate-pass，加 goal 工具的 Passive-complete 臂（Passive goal 不经 continuation hook，2026-07-24 补线）。三处全走 store CAS `try_claim_settle_notify`——章键 `(id, completed_at_ms)`，`completed_at_ms` 只在进入 Complete 的转移瞬间盖、离开即清，完成后的 lesson/note 编辑不能再燃 |
| objective 写保护 | `src/builtin_tools/goal.rs` (Set/Clear) | 被 `owns_reference` 治理的 goal：set 替换/clear＝拒绝+指路提案 note；逃生口＝用户确认后 unlink→改→relink（provenance 留痕） |
| prompt 层 | `src/thinker/layers/graph_topology.rs` @1754（紧邻 `TimerLoopLayer` @1753 之后） | 被治理会话逐轮被告知其拓扑位置+根参照原文；图不变→字节不变（cache 安全）；非图内会话零注入 |
| root/frozen 人闸 | `src/config/types/policies/exec_tier.rs::asks_for_arguments` | Auto 档下 `loop_graph` 触及 `root:`/`frozen:` 的写调用参数级强制审批卡（复用 `src/tools/scoped/` 唯一强制点；背景会话无审批通道→fail-closed）。**同一个谓词也是「举不起审批卡的 surface」的地板**：`security::dangerous_tools::is_denied_on_gateway_surface` 现在收参数并直接读它，`tools.invoke` 与 heartbeat probe 共用——此前那个函数只看工具名，而 `loop_graph` 既不在 `DANGEROUS_TOOLS`（它不在 `BUILTIN_TOOL_DEFINITIONS` 里，加进去会红 `every_entry_names_a_real_tool`）也不 `requires_confirmation`，于是一次 `tools.invoke {action:"node", kind:"root", origin:"human", body:…}` 就能改写那份逐轮注入每个被治理会话 prompt 的根参照原文。在这个谓词里加规则＝同时收紧三处，别在别处再列一张名单。**作用域的诚实边界**：`enable_audit`（向每个 frozen 节点扇出 `audits` 边）与 `gc`（删结构死行）**刻意在闸外**——两者参数里都不带受保护 id，参数级机制表达不了；且前者纯增量、从不改 frozen 的 body，后者只清审计模板明确获授权清的东西。要收紧就在工具里加 `confirm` 参数，**别**让这个纯配置谓词去读 store。**round 11 扩了一格（2026-08-03）**：`unlink` 且 `edge == "owns_reference"` **也举卡**——这条边的两个端点都不带受保护前缀（被治理的是 `goal:<session>`，治理者是普通 `cron:`/`daemon:` 节点），于是 §6.2 objective 写保护此前可以被**它治理的那个环**用**一次不举卡的调用**摘掉，而所需参数正是 `builtin_tools/goal.rs` 拒绝时逐字印给模型的那一行。本文第 39 行一直把逃生口写作「**用户确认后** unlink」——这就是那个确认。⚠️ **这与 round 9 记下的「要收紧就在工具里加 `confirm`，别让这个纯配置谓词去读 store」不冲突**：那条规则的理由是 `enable_audit`/`gc` 的**参数根本表达不了**受保护 id，而 `unlink owns_reference` 的参数（`action` + `edge`）完全表达得了，实现也没有读 store。审计中有一位评审据前半句主张此项应放进工具，理由已在此逐条记下，裁定为保留在参数级谓词（同一改动因此自动收紧 `tools.invoke` / heartbeat probe 两个举不起卡的 surface——在没有人的通道上解除治理，正是该拒的）。**残余**：Full 档按其契约不闸（用户显式选择全信任），见 spec §11。<br>⚠️ **两条同批修的、不在本层但承载本层唯一人闸的缺陷**：(a) `sandbox/exec_approval/action.rs::preview` 按 `BTreeMap`（字典序）渲染并在 **200 字符**处截断，于是 `loop_graph` 被闸的标识符排在**无上限的模型自撰散文之后**（`to_id` 最末、`id` 在 `body` 之后）——一次带长 `note` 的 `link` 会举出一张 `to_id=root:aleph` **被整段截掉**的卡，理由行还是通用的「Tool `loop_graph` requires your confirmation」，而 `grant_fingerprint` 随后把会话授权绑到这次调用上；现在 `loop_graph` 与 `file_ops` 一样走**标识优先**的形状，截断只能吃掉散文。(b) `tools/scoped/builder.rs::tier_asks_for_arguments` 的「操作者显式点名了这个工具就不问」用的是**会匹配 glob 的** `resolve_explicit`，于是一句 `"*" = "allow"`（"别再打断我"的常见写法）静默关掉 `file_ops delete` 与 root/frozen 写面前**最后一张卡**；判据改为**精确点名**（glob 仍然照常决定该工具本身的 allow/ask/deny）。(c) 参数级闸判的是 `input`，真正执行的是 `effective_input`，中间隔着 `BeforeToolCall` 的 `update_input:`；仅在 hook **确实改写了参数**时重判一次（没有 hook / 没改写＝零行为变化） |
| channel 层 operator 闸 | `src/gateway/method_authz.rs::OPERATOR_TOOLS` | `loop_graph` 在列：它装 cron job（`enable_audit`/`pair`，正是 `cron_manage` 被 operator 闸的能力），且 root body 是持久 prompt 注入面。此前唯一护栏是 Auto 档参数卡，而在 channel 上那张卡问的人**就是发起请求的 chat-tier 参与者** |
| doctor 体检 | `src/diagnostics/checks/loop_graph.rs` | `core/loop-graph` 只读结构 lint——审计节奏之外的廉价即时观测面；**刻意无机械修复**（悬空边归审计环裁决）。**「没问题」与「什么都没声明」是两个答案，只有一个令人安心**（2026-08-03 round 11）：daemon 启动时无条件建 `loop_graph.db`，所以 `!path.exists()` 那条分支**在生产里是死的**，一张**空图**（零节点＝零看守、零锚点、零根参照）此前报「Topology sound」——治理层给自己的缺席发合格证，正是它要抓的那类失败。空图现在单独报 `No topology declared` |
| skill（教义） | `Aleph-skills/loop-governance/` | 词表语义、配对指南、真伪锚点表、审计模板、Manheim & Garrabrant 四型标注 |

## 4. 四失败 → 落地机制

1. **Goodhart → `watches` 配对**：`pair` 语法糖一次建看守 cron+边；goal 的胜利宣称即时触发看守（post-run 钩子+去抖），周期 cadence 兜底。反指标选择是认知，归 LLM——代码不自动生成看守。首发 dogfood：dreaming 蒸馏产量 × 用户纠正率。
2. **参照盲区 → `owns_reference` 层级**：治理环拥有子环 objective；子环对自己的参照只读（goal 工具 ACL）；变更走提案 note→治理环慢 tick 裁决→用户确认。`reference_owned_by` 链必须汇于 root（lint 检查）。
3. **冲突 → `arbitrates` 仲裁**：检测靠 LLM（永不建冲突检测器，R7 红线），仲裁是事件不是服务；准绳向上锚到 root 根参照。
4. **测量衰减 → `audits` 审计环**：独立周 cron（enable_audit 安装；**明确拒绝 dreaming 托管**——被审对象的宿主不能当审计员），七步：取拓扑→锚点真实取证（mode=ro）→报表对现实对账→验尸探针与冻结节点→点名（剧场循环/裸奔环/悬空边）→裁决 note→上报。层级封顶三层：优化环→审计环→**人**（读 digest，无元审计环）。

## 5. 锚点 / 冻结 / 根参照

- **真锚点**（body 声明 `{probe, truth}`）：真实跑过的测试退出码、`aleph://correction/` 用户真实纠正、真实 token 消耗、CI 结论、heartbeat 探针原始输出。**伪锚点明拒**：LLM 自我报告、美元估算、`memory_health_score` 类管线自产指标、reference-free judge 裸分。锚点命令经既有 exec 工具执行（sandbox 硬底线原样生效）——图层无执行代码，不可能成为旁路。
- **冻结节点**：登记+审计巡检，执法留在原地（budget.rs 棘轮、sandbox 硬底线、scoped 唯一强制点、TLS fail-closed、EditBudget）。判据：凡优化器想松动的规则就是该冻结的规则。
- **根参照**：`root:*` 节点 body=人写原文；三重结构化——store 级 origin=human 不变量、Auto 档参数级审批卡、（Phase 0 期间）`~/.aleph/soul.md` 根参照节声明任何自动过程不得改写。

## 6. 运维（Phase 0 宪章 · 2026-07-19 落地）

- 周审计 cron 已在生产 daemon 运行（`0 0 10 * * MON` Asia/Shanghai）；`loop-governance` skill 已装（`~/.aleph/skills/`）；heartbeat 反指标探针待 daemon 重启后创建（`[heartbeat] enabled=true` 已翻）——手册见 [docs/superpowers/plans/2026-07-19-graph-phase0-runbook.md](../superpowers/plans/2026-07-19-graph-phase0-runbook.md)。
- 审计会话执行档位：需真实跑锚点命令，`Ask` 档会卡死无人应答的背景会话——审计 cron 建议 `Auto` 档 + sandbox 硬底线兜底。

## 7. NOT-build（长期有效，摘录）

Graph RAG/多跳图检索（记忆检索议题）；Neo4j 等图数据库（红线）；LangGraph 控制流图；图健康分（图自身不可被 Goodhart）；判决 schema 解析器/裁决执行器；确定性冲突检测器；自动生成看守；指标时序库/图内观测缓存（=报表对报表）；元审计环；FK 级联与自动 gc；champion-challenger 晋升管线（待自调策略消费者）；`src/harness/` 任何行。完整清单见 spec §10。

### 参考项目对照（LangGraph · Gap Analysis，2026-08-03）

**改 `src/workflow/` 或本层前先看这张表，不必重做对比。**

先厘清一件事：**LangGraph 对标的不是本层**。本层是「改进环之图」（谁看守谁，慢变、可审计）；LangGraph 是**单次执行内的控制流图**，Aleph 里对应的是 `src/workflow/` 的声明式 DAG（`WorkflowDef` → `compile.rs::materialize` → `coord_tasks` → `TeamDispatcher`）。两者不可互换，下表逐项判的是 workflow 层。

| LangGraph 机制 | Aleph 对应 | 处置 |
|---|---|---|
| Durable execution / checkpointer（每 super-step 存快照） | `coord_tasks` + `coord_task_runs` 落 SQLite；`Blocked`/`Unsatisfiable` 读时派生，调度器无内存态 | **SKIP（已有且更彻底）**：没有"内存态需要快照"这回事 |
| Pending writes（失败节点的成功兄弟不重跑） | 每 step 一行，失败只重跑自己 | **SKIP（已有，粒度更细）** |
| Human-in-the-loop `interrupt()` | `WorkflowStepKind::Clarify`（park→投递→router 认领，pending/delivered 两态章 + 重投 janitor） | **SKIP（已有，且多了投递可靠性状态机）** |
| 容错重启 | `tasks/retry.rs` 有界重试 + 指数退避 + jitter + recovery context 续做 | **SKIP（Aleph 更强）** |
| Time-travel / fork（回到任意检查点重放、分叉） | 无（只有逐 step retry） | **SKIP（YAGNI）**：`workflow(action='run')` 重跑一次即可；fork 的消费者不存在 |
| **Conditional edges / router** | 无（`depends_on` 静态 DAG，`topo_order` 硬拒环） | **DECIDE（人来定，勿自行开工）**：缺口属实，但**不是红线可清的**——spec §10 item 3 「LangGraph 式控制流图…把认知编进图结构 = fat harness，违 R10」是**通则**，并未把 `src/workflow/` 列为豁免地。今天可用的替代：`workflow_step_review{action:'skip'}` 手动剪枝（`Skipped` 已满足下游依赖） |
| **运行时宽度扇出/扇入** | coord_task 的依赖集**在 INSERT 时冻结**（`crud.rs` 是唯一写者，无 `add_dependency`）；节点内可用 `subagent{batch_tasks, synthesize}` 做模型定宽的菱形 | **DEFER**：要一个 append-only `add_dependency`（`dag::check_no_cycle_sync` 守 + 目标离开 Pending/Blocked 即拒）。有真实用例再做 |
| **过期传播**（重跑上游使下游失效） | ~~无~~ → **已连线**：`workflow_step_review{retry}` 对已结算步骤返回 `now_stale`（`get_dependents` 的第一个消费者） | **DONE（2026-08-03，报告不代劳——是否重做归 LLM 判断，R7）** |
| 声明式路径要求锚点 | ~~`require_grounding` 只有 `task_create` 能设~~ → **已连线**：`WorkflowStepDef.require_grounding` | **DONE（2026-08-03）** |
| 子图嵌套（sub-workflow） | 无（`WorkflowStepKind` 只有 `Agent|Clarify`；运行时 `run` 另铸 run_id，status/cancel/settle 看不见子 run） | **DEFER**：正解是 `materialize` **编译期**展开（前缀化子 step id、重接边），不是运行时嵌套。等真有共享形状再做 |

### 参考项目对照（codex Multi-agent V2 · Gap Analysis，2026-08-02）

**改这一层前先看这张表，不必重做对比。** 双 checkout 复核（`80b65e9945` 与 `2b5bdcf675`），结论一致：

**codex 没有治理拓扑这个概念。** 它有的是一棵 spawn/provenance 树 —— `state/migrations/0021_thread_spawn_edges.sql`（`parent_thread_id`/`child_thread_id`/`status`），一个隐含动词 "spawned"，边状态只有 `Open|Closed`；加上 `core/src/agent/control.rs` 的生命周期管控（spawn/resume/interrupt/close/shutdown_tree）、`registry.rs` 的深度与并发上限、`protocol/src/agent_path.rs` 的 `/root/...` 层级命名、以及 `core/src/guardian/` 一个复核审批请求的 LLM 子代理（策略是 markdown，不是代码）。**没有**锚点、冻结规则、人供给根参照、结构 lint，也没有 audits / arbitrates / owns_reference 这类关系。较新 checkout 的 migrations 0022–0046 与新 crate `rollout-trace/`（`InteractionEdgeKind` 五词闭集 + `TraceAnchor` + 边上带 `carried_item_ids` 证据指针）**是离线事后 debug 图** —— 描述性的会话史，不渲染进 prompt、对任何环没有权威，与 `thread_spawn_edges` 同类，只是更丰富。

| 机制 | codex | Aleph | 处置 |
|---|---|---|---|
| 拓扑存储 | 1 动词 3 列的 spawn 边 | 六词闭集 + 节点种类 + provenance | ALEPH-AHEAD |
| 锚点 / 冻结 / 人供给根参照 | 无 | `NodeKind` + `Origin` + store 级不变量 | ALEPH-AHEAD |
| 结构 lint | 无 | `store::lint` + doctor | ALEPH-AHEAD |
| 逐轮拓扑进 prompt | `session/mod.rs` 注入活跃子代理 | `graph_topology.rs` @1754 | SKIP（已有；且注入点在 R10 锁区） |
| LLM 复核者 | `guardian/` | 审计环 / 看守环模板 + `loop-auditor` | SKIP（已有，策略同样在 prompt 侧） |
| 边上带证据指针 `evidence_ref` | `rollout-trace` `carried_item_ids` | `origin` + 散文 `note` | **SKIP（YAGNI）**：新字段零消费者 |
| 边软关闭 `ended_at` 取代删除 | `ThreadSpawnEdgeStatus::Closed` | 删除 + 悬空即审计信号 | **SKIP（冲突）**：与「悬空边是审计信号、只由显式 `gc` 清」直接对立 |
| 拒绝次数熔断 | `GuardianRejectionCircuitBreaker`（连续 3 / 近 10 次即确定性推翻模型裁决） | — | **不移植（违 R7）**：在模型裁决之上叠打分启发式 |
| 按文本匹配剥离提示 | `is_multi_agent_v2_usage_hint_message` | — | **不移植（违 R7）**：对模型输出做意图分类 |
| 确定性模型选择 | `find_spawn_agent_model_name` | — | **不移植（违 R7）** |
| `AgentControl`/`AgentRegistry` 接进轮次组装 | `session/mod.rs` | — | **不移植（违 R10）**：属 harness 级会话管线，要走 `src/loop_graph/` + 一个 builtin 工具 |

**结论：本轮无一项可移植。** §4.12 这一轮的全部价值来自对 Aleph 自身这一层的深度审计（9 项缺陷，见 FEATURE_LOCATOR §4.12 Round-9）。

### 参考材料对照（《什么是图工程》· 2026-08-03 round 11）

《From Loop Engineering to Graph Engineering》系列的中文长文（`/Volumes/TBU4/技术文章/什么是图工程-纯文本.txt`）与本层的关系**只有一条**，但那一条是本层存在的理由：

> 「虽然图允许任务怎么拆、怎么合现场灵活调整，这叫**工作图**，可以快速变化；但是谁有权改数据库、谁能绕过审批这类长期权限，绝不能让模型现场发挥，这叫做**角色图**，必须**慢变、可审计**。」

**Aleph 里这是两个不同的子系统，不要混**：工作图＝`src/workflow/`（声明式 DAG → `coord_tasks`，快变，每次 run 重建）；角色图＝**本层** `src/loop_graph/`（慢变、独立 DB、优化器无写权限、结构 lint + 人供给根参照）。文章其余的全部内容——菱形扇出扇入、主管-工人、流水线、路由、评估-优化、验证器、LangGraph checkpointer / time-travel / pending writes——**讲的都是工作图**，逐项处置见上一张 LangGraph 表。

| 文章主张 | 本层现状 | 处置 |
|---|---|---|
| 角色图必须慢变、可审计，长期权限不许模型现场发挥 | 六词闭集 + `origin` provenance + root origin=human 不变量 + Auto 档参数卡 + `lint` | ✅ 本层就是这条 |
| 「让模型的判断力落在节点上，让代码的可靠性落在边上」 | 边＝结构（代码只校验拓扑），节点内的裁决＝一次普通 LLM turn（`templates.rs` + skill） | ✅ 已是 R7/R9 的形状 |
| 图必须接地：锚点得是无法狡辩的硬事实（测试真跑过 / 用户真留下 / 钱真到账） | `NodeKind::Anchor` + body 强制声明 `{probe, truth ∈ exit_code\|numeric\|line_count}` + 伪锚点明拒清单（§5） | ✅ 已有，且比文章更严（拒 LLM 自我报告 / 美元估算 / 管线自产分数） |
| 「更好」到底指什么必须由人来定 | `root:` 节点 body ＝人写原文，机器在通道上不可达（store 不变量 + 审批卡 + 逐轮注入） | ✅ 本层的独有部分——文章只提出要求，没给机制 |
| 验证器（Verifier）是性价比最高的节点，专门试图推翻前一个结论 | 审计环 / 看守环模板 + `loop-auditor`（`ContextMode::Fresh` 零继承） | ✅ 已有；**但那是慢环之间的复核**，run 内的对抗验证属工作图，见 `src/workflow/` |
| 路由器（Router）按重要程度分诊到不同检查强度 | 无，且**不建** | ❌ 违 R7：按内容判"这件事多重要"是模型的活；今天的等价物是模型自己决定要不要 `pair` 一个看守 |
| 检查点 / 时间旅行 / pending writes | 工作图议题 | 见 LangGraph 表（SKIP/DEFER） |
| 多智能体 token ≈ 普通对话 15 倍，只在价值够高时用 | — | ✅ 与 R10「先找最简单的方案」同向；本层零常驻成本（空图 ⇒ prompt 零字节） |
| **V/E/S/P 里的 S**：状态沿边流动，下游只拿干净的结构化产出，不看上游的原始垃圾 | 工作图议题：`teams/dispatcher/handoff.rs::build_handoff_context` 把上游 step 产出注入下游 step 的 prompt（`compile.rs` 模块 doc 与 `WorkflowStepDef::prompt` 的 doc 都指名它） | ✅ 已有（**2026-08-10 round 12 补录**：此前七行表漏了这一行，不是缺口而是漏记） |
| V/E/S/P 里的 **P**（谁能建节点/调工具/改图） | 角色图议题＝本层 | ✅ 本层就是 P |

**结论：本轮同样无一项新机制可移植**，文章的价值是确认了分层判据（角色图 vs 工作图），而 round 11 的八项缺陷全部来自对本层自身的对抗审计。

### Round 11（2026-08-03）—— 八项，逐项见 FEATURE_LOCATOR §4.12

一句话总结这一轮的形状：**round 9 与 round 10 修的是覆盖环的形状，round 11 修的是「谁有资格进入这张图的权力位置」以及「这张图对 prompt 说的话有没有兑现能力」。** 三条可跨子系统复用的纪律已提到 CLAUDE.md。

### Round 12（2026-08-10）—— 十七项，逐项见 FEATURE_LOCATOR §4.12

一句话总结这一轮的形状：**round 11 之后全仓做了多用户改造（P0/P1/P2 + round 2-3，2026-08-07~09），而本层的上一次审计早于它——所以 round 12 修的是「这一层在一个它从未被审过的世界里还成不成立」。**

新增的一条参考材料结论：《什么是图工程》的 **V/E/S/P 四元组里的 S（沿边流动的共享状态）此前不在任何一张对照表上**。复核结论是**已有**——`teams/dispatcher/handoff.rs::build_handoff_context` 把上游 step 的产出注入下游 step 的 prompt，`compile.rs` 与 `def.rs` 的 doc 都指名它。表格补一行，不是功能缺口。

三条本轮新得、可跨子系统复用的纪律：

1. **一道「升级给别人裁决」的闸，只有在发起者答不了它的时候才是闸。** `check_operator_gate` 不拒绝、而是举卡给 operator，但卡是按**发起者自己的 session_key** 登记的，而 `exec.approvals.pending` / `exec.approval.resolve` 对 member 是开的 ⇒ member 自批。判据：**写下一个「转交给更高权限」的分支时，去看那张卡最后落在谁手里。**
2. **闸的范围必须覆盖能把闸拿掉的那个动词——包括「用另一个工具改配置」这条路。** `explicitly_named` 的正当性建立在「这条 override 是人写的」，而 `self_config` 写 override **不举卡**：两步都合法，合起来等于永久摘掉 root/frozen 的唯一人闸。
3. **判据钉在一个「每轮都会改」的常量上，等于钉在空气上。** `enable_audit` 的孤儿认领比对 `job.prompt == AUDIT_TEMPLATE`，而 job 的 prompt 是安装当天写死的、模板此后改了五次 ⇒ 对**存在最久**的那个环恒假，而它正是这段代码要认领的对象（新装的反而匹配，所以 fixture 全绿）。判据：**比对用的那个值，两边是同一时刻写下的吗？**

### Round 13（2026-08-14）—— 事件总线 / 快照 / 只读门面 / 导出，四个模块零行为变化

一句话总结这一轮的形状：**前十二轮修的是这张图说的对不对，这一轮补的是这张图说的话有没有人听、历史能不能回放、移除前能不能预演。** 全部增量，无一项改动既有不变量或裁决路径；参考项目对照见上文 §7（codex / LangGraph /《什么是图工程》长文三张表，本轮无新移植——结论不变）。

1. **拓扑事件总线（`events.rs`）**：`tokio::sync::broadcast`（cap 256），五种事件（NodeUpserted/NodeDeleted/EdgeUpserted/EdgeDeleted/GcCompleted）。此前四个读者（prompt 渲染 / cron poke / objective ACL / doctor lint）互不知道对方存在，`pair` 装完看守没有任何机制通知 prompt 层「你的现实刚变了」。bus 缺席时 `publish` 为 no-op（测试/早期 boot 零特判）；工具写面九个变更点全部在**成功之后**发布。**刻意不发布拒绝事件**（store 拒绝非法边不产生事件——那是写屏障的活，见 NOT-build）。
2. **时间快照（`snapshot.rs`）**：独立 `loop_graph_snapshots.db`（与拓扑 DB 同域但不同文件——快照是审计轨迹，不应加宽治理 store 的写锁面）。`capture`/`list`/`get`/`diff`/`delete`；diff 按稳定键 BTreeMap、Added↔Removed 对称、排序确定性（审计日志依赖可重现 diff）。**刻意无 restore**——快照是记录不是回滚原语，恢复会把 operator 故意删掉的行（如已知治理者已死的 owns_reference 边）静默复活。踩过的坑：`rusqlite execute()` 返回受影响行数不是 rowid，快照 id 必须用 `last_insert_rowid`——否则每个 diff 都读出同一行而恒为空（测试抓到）。
3. **只读查询门面（`inspector.rs`）**：`subgraph_for`（沿 owns_reference 双向走 ancestor/descendant，1024 步硬顶——防意外成环跑死）、`impact_of_removing`（**纯模拟**：post-state 保留悬空边——真实 `drop_node` 不级联，模拟若假装级联就藏起了它存在的意义——预览悬空边发现）、`summary()`、`loops_with_coverage()`。coverage 过滤与 lint 共用同一 `can_cover` 谓词（anchor/frozen 不算看守）。
4. **导出（`export.rs`）**：`to_dot`（Graphviz 按 kind 分形状颜色，body 只进 tooltip）+ `to_json`，确定性字节排序。零新依赖。
5. **工具面三个只读 action**：`impact`（drop_node 前的爆炸半径，消息明说「纯模拟」）/`export`（dot|json）/`snapshot`（capture|list|diff）。棘轮纪律照守：DESCRIPTION +84 B、schema +334 B 均先削后抬——同批从 loop_graph 自身删掉 620 B schema 赘肉（枚举已列出的动词清单、交叉引用、冗长限定语），definitions.rs 两个 ceiling 常量各附三问论证。
6. **连线测试**：`node_action_publishes_upsert_to_global_bus` 用唯一 id 过滤并行测试的兄弟事件，端到端证明 tool → 全局 bus 通路。
7. **顺带修复（先于本轮、阻塞全仓编译）**：main 最新提交删除 `interfaces/webchat/dist/.gitkeep`，而 `.gitignore` 明确注释该 sentinel 是 RustEmbed 编译期依赖（无目录则 `ControlPlaneAssets::get()` 不生成，E0599 全仓红）。已恢复。

## 多智能体融合（2026-07-19 第二轮，spec: specs/2026-07-19-graph-multiagent-fusion-design.md）

- **独立视角**：审计/看守模板默认 `subagent(agent_type="loop-auditor")` 独立取证（builtin agent：`ContextMode::Fresh` 零继承、READ_ONLY+bash+`governance_metrics`（2026-07-24 补——sandbox 使 bash 摸不到 `~/.aleph/data`，模板点名的常备探针此前必败；`cron_manage`/`loop_graph` 刻意不加，前者含写动作、后者能改图）、denied file_write/file_edit/search/web_fetch）——治「共读同套数据互证正确」。落点 `src/agents/registry.rs` + `src/loop_graph/templates.rs`。
- **Team 显式入图**：`NodeKind::Team`（`team:<id>`）只经显式 node/pair 进图，快环 coord task 永不进表；status live-join `TeamStore`；`team_disband` 成功即胜利宣称 → `notify_team_settled` poke 看守（60s 去抖，与 goal 同内核 `notify_node_settled`）。落点 `src/loop_graph/{types,service}.rs` + `src/builtin_tools/{loop_graph_manage.rs,team/disband.rs}`。
- **Grounding 进执行层**：`task_create(require_grounding=true)`（acceptance metadata 通道，零迁移）→ `task_review` approve 无 `grounding` 证据即 bounce（`grounding_required`）；证据 kind 闭集与 anchor truth 同词表（exit_code|numeric|line_count），以 `[grounding]` comment 存证供审计环核验。reject 永不要求锚（拒绝天然保守）。落点 `src/agents/swarm/tasks/acceptance.rs` + `src/builtin_tools/team/task_review.rs`。
- **编排智慧**：leader prompt 三教义（防过度编排 / 审查独立触地 / 失败局部重跑），`src/teams/leader_prompt.rs`。
