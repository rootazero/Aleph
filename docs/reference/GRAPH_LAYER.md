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
| 类型（6 词闭集边） | `src/loop_graph/types.rs` | `NodeKind`(loop_goal/loop_cron/loop_heartbeat/daemon/anchor/frozen/root)、`EdgeKind`(watches/owns_reference/arbitrates/audits/anchored_by/feeds)、`Origin`(human/llm，provenance 一等) |
| 存储 | `src/loop_graph/store.rs` | 两表（graph_nodes/graph_edges，agent_id 作用域，PK 复合）；`open_sqlite_safe`；**root origin=human 是 store 级不变量**；无 FK 级联（悬空边=审计信号，显式 `gc` 才清）；`lint()` 纯结构检查（悬空/裸奔优化环/治理链未锚定 root/快环拥有慢环参照） |
| 模板（智慧在此，R9） | `src/loop_graph/templates.rs` | `AUDIT_TEMPLATE`（七步审计）/`WATCH_TEMPLATE_*`（看守）/`STEWARD_TEMPLATE`（参照治理）/`ARBITRATION_TEMPLATE`（仲裁） |
| 触发与会话服务 | `src/loop_graph/service.rs` | `notify_goal_settled`（胜利宣称时刻戳看守 cron，60s 去抖）、`governing_owner`（objective ACL 查询）、`render_session_topology`（prompt 注入渲染，**确定性字节**） |
| 工具（R8 面） | `src/builtin_tools/loop_graph_manage.rs` | `loop_graph`(action: node/drop_node/link/unlink/list/status/gc/enable_audit/pair)；anchor 强制 body 声明 truth∈{exit_code,numeric,line_count}；status 做 live join（goal store/cron jobs 实时状态，永不缓存观测） |
| 胜利宣称触发点 | `src/gateway/execution_engine/goal_continuation.rs` | gateless-terminal-complete 与 gate-pass 两处调 `notify_goal_settled` |
| objective 写保护 | `src/builtin_tools/goal.rs` (Set/Clear) | 被 `owns_reference` 治理的 goal：set 替换/clear＝拒绝+指路提案 note；逃生口＝用户确认后 unlink→改→relink（provenance 留痕） |
| prompt 层 | `src/thinker/layers/graph_topology.rs` @1753 | 被治理会话逐轮被告知其拓扑位置+根参照原文；图不变→字节不变（cache 安全）；非图内会话零注入 |
| root/frozen 人闸 | `src/config/types/policies/exec_tier.rs::asks_for_arguments` | Auto 档下 `loop_graph` 触及 `root:`/`frozen:` 的写调用参数级强制审批卡（复用 `src/tools/scoped/` 唯一强制点；背景会话无审批通道→fail-closed）。**残余**：Full 档按其契约不闸（用户显式选择全信任），见 spec §11 |
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
