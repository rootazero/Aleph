# Graph 层 × 多智能体融合设计（Loop-Graph × Multi-Agent Fusion）

- **日期**: 2026-07-19（同日承接 `2026-07-19-graph-engineering-loop-graph-layer-design.md`，为其第一次深化轮）
- **动机文本**: 用户提供的第三篇文章——Loop→Graph 范式变迁：单 Loop 只能看见自己被要求优化的东西（四类失败）；Graph = 多个不同功能、不同速度的 Loop 彼此连接、监督、否决、纠错；但共读同一套数据的 Graph 只是互证正确，真正重要的是 Grounding（不可自改的锚点 + 冻结规则 + 人供价值判断）；Graph 的价值在把状态、分支、失败恢复显式化，最大的坑是把简单任务过度编排。用户点题：**"graph 启动的就是多 agent，充分利用 multiagent 模块"**。
- **现状**: `src/loop_graph/`（治理拓扑，7-19 落地）与多智能体模块（Spawn/Delegate/Team，`src/agents/` + `src/teams/`）互不相通；审计/看守环在 own-session 跑 cron LLM 回合——与被审计者共读同一套记忆与上下文，恰是文章点名的"互证正确"隐患；`subagent_spawn` 的独立 context（`ContextMode::Fresh`）是现成解药。

## 0. 决策记录（用户四问四答）

| 决策点 | 选择 |
|--------|------|
| 优化主轴 | 四方向全做：治理环独立视角化 + 执行图纳入治理图 + 防过度编排编排智慧 + Grounding 进 Team 执行 |
| Team 入图耦合深度 | **显式选择才入图**——默认 Team 运行零接触治理图；显式 pair 时 `team:<id>` 才成节点；快环 task 永不进治理表 |
| 独立视角机制 | **模板 + 官方 loop-auditor agent**——结构性独立 context + 结构性只读倾斜，不硬强制（R7） |
| Grounding 力度 | **可选字段 + 开关硬化**——`task_review` 加可选结构化 grounding；per-task `require_grounding` 开关；不逼无锚任务造假锚 |

## 1. 总架构

```
慢环（治理，loop_graph 表，持久）          快环（执行，Team/CoordTask，临时）
┌─────────────────────────────┐          ┌──────────────────────────────┐
│ root:aleph（人供根参照）      │          │ Team: leader + members       │
│ cron:audit ─audits→ …       │          │  CoordTask DAG（局部重跑已有）│
│ cron:watch ─watches→ team:X ←┼─显式pair─┤  task_review（+grounding 新）│
│ anchor:*（不可辩驳测量）      │          │  disband = 胜利宣称 ──poke──→│
└──────────┬──────────────────┘          └──────────────────────────────┘
           │ 审计/看守回合执行时
           ▼
   subagent_spawn("loop-auditor")   ← 独立 context（Fresh）+ 可测量不可改写
```

四个工作包全部落在工具层 / prompt 层 / agent 定义层。`src/harness/` 零触碰（R10），无新依赖、无新库（R3），全部裁决仍由 LLM 回合完成（R7/R9）。

**事实修正（本轮侦察发现）**: `docs/reference/MULTI_AGENT_SYSTEM.md` 的 Role Mechanism 章节描述的 `review_score` / `ReviewScore` / `TeamRoleConfig` / `min_challenges` **不存在于实现**。实际审查机制 = `src/builtin_tools/team/task_review.rs`（leader approve/reject，记 `ReviewVerdict`/`ReviewerKind` 入 task run）；task 级 policy 走 `src/agents/swarm/tasks/acceptance.rs` 的 metadata JSON 通道（`lead_review_required` / `acceptance_criteria` 模式）。本设计基于真实落点，并在 WP4 修正该文档漂移。

## 2. WP1 · 治理环独立视角化（~80 LOC）

**问题**: 审计/看守 cron 在 own-session 执行，取证与裁决共用被审计者的记忆与上下文——"所有 Agent 读同一套数据 → 彼此证明对方正确"。

**方案**:

1. `src/agents/registry.rs::builtin_agents()` 新增官方 **`loop-auditor`** AgentDef：
   - `AgentMode::SubAgent`；`ContextMode::Fresh`（默认值即零上下文继承——注意代码中不存在文档所称 "standalone"，`Fresh` 即所需语义）
   - 允许 `INVESTIGATION` 工具集 + `bash`（锚点测量需要真实退出码/独立测量值）
   - denied `file_write` / `file_edit`——**可测量、不可改写**。bash 的残余写能力由既有 exec tier + `[sandbox.command_policy]` 硬底线兜住，不另造沙箱（防御性倾斜，不追求完美隔离）
   - `when_to_use` 明确指向治理场景（审计环/看守环取证），避免污染日常 spawn 选型
2. `src/loop_graph/templates.rs`：
   - `AUDIT_TEMPLATE` 七步中的取证步骤改写为「先 `subagent_spawn(agent="loop-auditor")` 独立取证（探针复跑、锚点测量、数字复核），主回合只做裁决与落 note」
   - `WATCH_TEMPLATE_HEADER/FOOTER` 反指标测量同理
   - 不硬强制：R7——简单情况模型可自行判断免 spawn；模板把独立视角设为**默认路径**

## 3. WP2 · Team 显式入图（~200 LOC）

**问题**: 治理图不认识多智能体执行结构；Team 的"宣布完成"（disband）没有外部复核时刻。

**方案**:

1. `src/loop_graph/types.rs`：新增 `NodeKind::Team`（id 前缀 `team:<id>`；cadence 恒 `None`——team 无节奏概念，`cadence_rank` 不适用）
2. `src/builtin_tools/loop_graph_manage.rs`：
   - `expected_prefix` 加 `NodeKind::Team => "team:"` 臂
   - `status` 动作 live-join `TeamStore`（名称/状态/成员数，同 goal/cron 现行 live-join 做法）
   - `gc` 认 disbanded team 为死实体（悬空边保留=审计信号，同现契约）
   - `pair` 动作的 target 支持 `team:<id>`（看守 cron 模板措辞按 target 类型分支）
3. `src/loop_graph/service.rs`：新增 `notify_team_settled(team_id)`——复用 60s DEBOUNCE 与 `CRON_TRIGGER`，查 `watches → team:<id>` 边并 poke 看守 cron。best-effort no-op 契约同 `notify_goal_settled`
4. 挂接点：`src/builtin_tools/team/disband.rs::call()` 成功路径——**team 解散即胜利宣称**，正是便宜胜利该被复核的时刻。触发失败不阻塞 disband 主流程
5. **不做**：team 成员会话注入 `GraphTopologyLayer`（deferred——leader 已有 `Team.protocol` 通道可自行传达被看守事实）；Team 自动入图；task 级节点

## 4. WP3 · Grounding 进 task_review（~150 LOC）

**问题**: Critic/leader 审查只读 submitter 的 artifact 即可放行——审查未必触到现实。

**方案**:

1. `src/builtin_tools/team/task_review.rs`：`TaskReviewArgs` 加可选字段

   ```rust
   pub struct GroundingEvidence {
       /// exit_code | numeric | line_count — 与 loop_graph anchor body 的
       /// truth 闭集同词表（全系统一套锚点语言）
       pub kind: String,
       /// 测量来源（真实跑过的命令 / 独立数据源标识）
       pub source: String,
       /// 测量值（退出码 / 数字 / 行数）
       pub value: String,
       #[serde(default)]
       pub note: Option<String>,
   }
   ```

2. 开关走 `src/agents/swarm/tasks/acceptance.rs` 既有 metadata 通道：新 key `require_grounding` + 读写 helper（复制 `lead_review_required` 模式：零迁移、per-task 粒度、restart 幸存）
3. 校验规则：`require_grounding=true` 且 `decision=approve` 且 `grounding=None` → 该次 review 调用被拒并附指导文本（教 leader 如何取证或改派 loop-auditor）。**reject 不要求锚**——拒绝天然保守，无 Goodhart 风险；无锚可举的任务（文案/设计评审）leader 不开开关即可，不逼造假锚
4. grounding 证据随 review 记入 task run 记录（`runs.rs` review 列扩展或 metadata，plan 阶段定精确列），供审计环 / loop-auditor 事后核验"审查是否触到现实"
5. leader 相关 prompt（`src/teams/leader_prompt.rs`）教导何时开 `require_grounding`

## 5. WP4 · 编排智慧（纯文本，零代码结构）

1. `src/teams/leader_prompt.rs` 增补三条教义：
   - **防过度编排**——目标明确的短任务用单 loop / 单 subagent；出现并行、审批、回滚、跨工具依赖才建 team（文章："最容易踩的坑，是把简单任务也过度编排"）
   - **审查者独立数据源**——不能只读 submitter 的 artifact 自证；须独立复核或要求锚点证据
   - **失败局部重跑**——reject 重做原 task（CoordTask DAG 已支持），不推倒重来
2. `loop-governance` skill（Aleph-skills 兄弟仓）增补「Graph × 多智能体」一节：何时上 team / 独立视角原则 / grounding 证据词表
3. 文档：
   - `docs/reference/GRAPH_LAYER.md` 加 WP1-3 落点与本 spec 链接
   - `docs/reference/MULTI_AGENT_SYSTEM.md` **修正 Role Mechanism 漂移章节**为实际的 task_review + acceptance metadata 机制

## 6. 测试与错误处理

- **types/store**: 新 NodeKind 前缀校验 / 序列化往返 / gc 认 disbanded team
- **service**: `notify_team_settled` 用 `_in` store-taking 变体测试（避开 OnceCell 全局竞争，同 `notify_goal_settled_in` 先例）
- **task_review**: 校验矩阵 `require_grounding × decision × grounding 有无`（4 关键格：开+approve+无锚=拒；开+approve+有锚=过；开+reject+无锚=过；关+approve+无锚=过）
- **acceptance**: `require_grounding` helper 幂等 / 非对象 metadata 提升（同 `lead_review_required` 测试形态）
- **降级契约**: 无图 / 无 store / 无 cron 句柄 / TeamStore 读失败 → best-effort no-op，永不阻塞 disband / task_review 主流程
- 实施顺序 WP1→WP4，单分支 main 分包提交，每包独立可合

## 7. NOT-build（本轮明确不做）

1. task 级治理节点（快环留在 CoordTask DAG）
2. Team 自动入图 / 自动投影 Critic 边（显式 pair 是唯一入口）
3. team 成员会话的 GraphTopologyLayer 注入（deferred，protocol 通道已够）
4. 审计 cron 的结构硬强制独立执行体（动 cron/loop 执行链，失去 own-session 裁决便利）
5. 全 review 硬性 grounding（逼假锚，Goodhart 反噬治理机制自身）
6. 新沙箱 / loop-auditor 完美只读隔离（既有 exec tier + command_policy 兜底）
7. 文档中虚构的 `ReviewScore`/`TeamRoleConfig` 补实现——按 YAGNI 修文档而非补代码

## 8. 红线与原则对照

| 红线/原则 | 本设计如何满足 |
|-----------|----------------|
| R3 核心轻量化 | 零新依赖；~430 LOC 全在工具/prompt/agent 定义层 |
| R7 LLM 主权 | 裁决全由 LLM 回合完成；spawn 与否模型自主；校验只做结构核验（字段有无），不做语义判断 |
| R9 智慧在 Prompt | 独立视角教义、防过度编排教义全在模板/prompt/skill |
| R10 薄 Harness | `src/harness/` 零触碰；12 文件/行数棘轮不动 |
| P6 YAGNI | 显式 pair 才入图；7 条 NOT-build；文档漂移修文档不补码 |
| P7 防御性设计 | 全部触发 best-effort no-op；grounding 词表闭集校验 |
