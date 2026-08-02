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
| 存储 | `src/loop_graph/store.rs` | 两表（graph_nodes/graph_edges，agent_id 作用域，PK 复合）；读写两侧都走 `utils::sqlite_open`（写 `open_sqlite_safe` / 读 `open_sqlite_readonly`，后者的 `busy_timeout` 让 doctor 不会把并发写误报成「Graph DB unreadable」）；**三条 store 级不变量**：root origin=human、**治理边禁自环**（`watches`/`audits`/`owns_reference` 的 `from_id != to_id`——否则一次 `link` 就能让 `lint_naked_loops` 对该环永久静默，即本层要检测的测量衰减本身）、**`origin` 写一次不再被 upsert 改写**（provenance 是审计模板要查的东西）；无 FK 级联（悬空边=审计信号，显式 `gc` 才清）；**`body`/`cadence` 省略即保留（`COALESCE`）**——工具侧两者都是 `#[serde(default)] Option<String>`，全量覆盖会让「改个 label 重登记一次 `root:`」把人写的根参照原文写成 NULL，而两个读者都只在 `Some(body)` 时渲染 root 行，于是那一行从此后每一个被治理会话的 prompt 里静默消失（要清空传 `""`）；**存在性只问 `node_ids_present`（直读 `id` 列）**——`row_to_node` 对未知 enum 文本 fail-soft 跳过是给读者防卡死的，拿它当 `gc` 的 DELETE 判据会把「读不懂」当成「不存在」，不可逆删掉一个仍然存在的节点的所有边（两个 enum 都 `#[non_exhaustive]`，降级运行就是触发条件），`lint_dangling_edges` 共用同一份；`lint()` 纯结构检查（悬空/裸奔优化环/治理链未锚定 root（**全路径 BFS**，多 owner 时只要有一条路汇于 root 即算锚定）/快环拥有慢环参照） |
| 模板（智慧在此，R9） | `src/loop_graph/templates.rs` | `AUDIT_TEMPLATE`（七步审计）/`WATCH_TEMPLATE_HEADER`+`_FOOTER`（看守）。**仅此两类**——原 `STEWARD_TEMPLATE`/`ARBITRATION_TEMPLATE` 零消费者已 CUT（2026-07-24，R10 YAGNI）：steward/arbitration 的教义活在 `loop-governance` skill，此类环按需用 `cron_manage` 手建（仲裁刻意是事件非常驻服务，勿建安装器） |
| 触发与会话服务 | `src/loop_graph/service.rs` | `notify_goal_settled`（胜利宣称时刻戳看守 cron，60s 去抖；**返回「有没有真 poke」**——一次性章必须等 poke 确认后才算花掉，否则「cron handle 没挂 / `run_job` 报错 / 看守不是 `cron:`」三条出路都会让这次完成**永远**等不到评审，因为 Complete goal 的 `completed_at_ms` 此后不再变）、`watcher_is_pokeable`（**只有 `cron:` 看守能被即时唤醒**——poke 就是 `CronService::run_job`；手接的 `heartbeat:`/`daemon:` 看守照样满足 `lint_naked_loops`、照样渲染成「有人看着」，图看起来健全而评审要等它自己的节奏，故 `link` 在写入那一刻就说明，唯一还来得及纠正的时刻）、`governing_owner`（objective ACL 查询，**返回 `Result`**——读不到图必须拒绝写入而不是放行：把 store 错误折成 `None` 等于一次 `SQLITE_BUSY` 就关掉 §6.2 写保护，无错误无日志）、`render_session_topology`（prompt 注入渲染，**确定性字节**） |
| 工具（R8 面） | `src/builtin_tools/loop_graph_manage.rs` | `loop_graph`(action: node/drop_node/link/unlink/list/status/gc/enable_audit/pair)；anchor 强制 body 声明 truth∈{exit_code,numeric,line_count}；status 做 live join（goal store/cron jobs 实时状态，永不缓存观测）；**无 `agent_id` 参数**——图恒作用于 `routing::DEFAULT_AGENT_ID`（旧的模型可传旋钮是只写的：`service.rs` 与 doctor 的每个读路径都硬编码 "main"，非 main 作用域的图零 watcher poke / 零 ACL / 零 prompt 注入，而 `pair` 仍承诺 poke，2026-08-01 撤回）；`enable_audit` 的「已存在」闸判的是**活的审计环**——判据是节点 body == `templates::AUDIT_NODE_BODY`（与写入点同一常量），**不是**「存在某条 `audits` 边」：`Audits` 是一等动词（审计环→任意节点），手接一条 `cron:ratchet -[audits]-> frozen:budget` 是被鼓励的用法，用边当判据会让装过这种边的图**永远装不上审计环**，而报错还教用户去 `drop_node` 一个无关节点（照做就毁掉自己的治理边）；同理照它自己的提示 `drop_node` 之后留下的悬空边也不该挡住重装。`status` 的 live join **区分「名册读了但没这条」与「名册读不到」**——一次 `list_jobs` 失败曾让每个 cron 节点都被打上「⚠ target missing」，而审计模板正是拿这行进点名步骤，一次瞬时错误就能对着一图健康的环制造审计发现 |
| 胜利宣称触发点 | `src/gateway/execution_engine/goal_continuation.rs` + `src/builtin_tools/goal.rs` | **三处**调 `notify_goal_settled`：continuation hook 的 gateless-terminal-complete 与 gate-pass，加 goal 工具的 Passive-complete 臂（Passive goal 不经 continuation hook，2026-07-24 补线）。三处全走 store CAS `try_claim_settle_notify`——章键 `(id, completed_at_ms)`，`completed_at_ms` 只在进入 Complete 的转移瞬间盖、离开即清，完成后的 lesson/note 编辑不能再燃 |
| objective 写保护 | `src/builtin_tools/goal.rs` (Set/Clear) | 被 `owns_reference` 治理的 goal：set 替换/clear＝拒绝+指路提案 note；逃生口＝用户确认后 unlink→改→relink（provenance 留痕） |
| prompt 层 | `src/thinker/layers/graph_topology.rs` @1754（紧邻 `TimerLoopLayer` @1753 之后） | 被治理会话逐轮被告知其拓扑位置+根参照原文；图不变→字节不变（cache 安全）；非图内会话零注入 |
| root/frozen 人闸 | `src/config/types/policies/exec_tier.rs::asks_for_arguments` | Auto 档下 `loop_graph` 触及 `root:`/`frozen:` 的写调用参数级强制审批卡（复用 `src/tools/scoped/` 唯一强制点；背景会话无审批通道→fail-closed）。**同一个谓词也是「举不起审批卡的 surface」的地板**：`security::dangerous_tools::is_denied_on_gateway_surface` 现在收参数并直接读它，`tools.invoke` 与 heartbeat probe 共用——此前那个函数只看工具名，而 `loop_graph` 既不在 `DANGEROUS_TOOLS`（它不在 `BUILTIN_TOOL_DEFINITIONS` 里，加进去会红 `every_entry_names_a_real_tool`）也不 `requires_confirmation`，于是一次 `tools.invoke {action:"node", kind:"root", origin:"human", body:…}` 就能改写那份逐轮注入每个被治理会话 prompt 的根参照原文。在这个谓词里加规则＝同时收紧三处，别在别处再列一张名单。**作用域的诚实边界**：`enable_audit`（向每个 frozen 节点扇出 `audits` 边）与 `gc`（删结构死行）**刻意在闸外**——两者参数里都不带受保护 id，参数级机制表达不了；且前者纯增量、从不改 frozen 的 body，后者只清审计模板明确获授权清的东西。要收紧就在工具里加 `confirm` 参数，**别**让这个纯配置谓词去读 store。**残余**：Full 档按其契约不闸（用户显式选择全信任），见 spec §11 |
| channel 层 operator 闸 | `src/gateway/method_authz.rs::OPERATOR_TOOLS` | `loop_graph` 在列：它装 cron job（`enable_audit`/`pair`，正是 `cron_manage` 被 operator 闸的能力），且 root body 是持久 prompt 注入面。此前唯一护栏是 Auto 档参数卡，而在 channel 上那张卡问的人**就是发起请求的 chat-tier 参与者** |
| doctor 体检 | `src/diagnostics/checks/loop_graph.rs` | `core/loop-graph` 只读结构 lint——审计节奏之外的廉价即时观测面；**刻意无机械修复**（悬空边归审计环裁决） |
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

## 多智能体融合（2026-07-19 第二轮，spec: specs/2026-07-19-graph-multiagent-fusion-design.md）

- **独立视角**：审计/看守模板默认 `subagent(agent_type="loop-auditor")` 独立取证（builtin agent：`ContextMode::Fresh` 零继承、READ_ONLY+bash+`governance_metrics`（2026-07-24 补——sandbox 使 bash 摸不到 `~/.aleph/data`，模板点名的常备探针此前必败；`cron_manage`/`loop_graph` 刻意不加，前者含写动作、后者能改图）、denied file_write/file_edit/search/web_fetch）——治「共读同套数据互证正确」。落点 `src/agents/registry.rs` + `src/loop_graph/templates.rs`。
- **Team 显式入图**：`NodeKind::Team`（`team:<id>`）只经显式 node/pair 进图，快环 coord task 永不进表；status live-join `TeamStore`；`team_disband` 成功即胜利宣称 → `notify_team_settled` poke 看守（60s 去抖，与 goal 同内核 `notify_node_settled`）。落点 `src/loop_graph/{types,service}.rs` + `src/builtin_tools/{loop_graph_manage.rs,team/disband.rs}`。
- **Grounding 进执行层**：`task_create(require_grounding=true)`（acceptance metadata 通道，零迁移）→ `task_review` approve 无 `grounding` 证据即 bounce（`grounding_required`）；证据 kind 闭集与 anchor truth 同词表（exit_code|numeric|line_count），以 `[grounding]` comment 存证供审计环核验。reject 永不要求锚（拒绝天然保守）。落点 `src/agents/swarm/tasks/acceptance.rs` + `src/builtin_tools/team/task_review.rs`。
- **编排智慧**：leader prompt 三教义（防过度编排 / 审查独立触地 / 失败局部重跑），`src/teams/leader_prompt.rs`。
