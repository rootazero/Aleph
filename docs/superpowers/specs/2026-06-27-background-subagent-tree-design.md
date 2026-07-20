# 后台子智能体树状图 — 深度重构与功能增强设计

> Date: 2026-06-27 ｜ Scope: 全垂直切片 Phase 0–4 ｜ Viz: 丰富版（hermes 全平价）
> 参考: `T:\Github\hermes-agent`（delegate_tool.py / agentsOverlay.tsx / subagentTree.ts）
> 关联红线: R1 / R3 / R4 / R7 / R10 / P4

---

## 1. 目标与范围

把 Aleph 的后台子智能体从"**仅父 LLM 可轮询的扁平进度**"升级为"**用户可见的实时多层级树状图**"：

- **后端**：为后台子智能体补齐父子身份（parent_id/depth/root），引入 typed 生命周期状态机，向 panel 外发带身份的 live 事件，提供冷启动树快照 RPC。
- **前端**：新建 Leptos 树状图视图——层级渲染 + 展开/折叠 + 状态字形 + **热度 heatmap + sparkline 深度分布 + 排序/过滤 + rollup 摘要行**（hermes overlay 全平价）。
- **熵减**：单一 Rust 树重建源（原生 + WASM 复用），不产生平行实现；清理本次重构造成的孤儿。

**非目标（本次不做）**：spawn-tree 落盘历史快照回放（复用 trace_replay 基建，留 Phase 5）；同步（前台）子智能体进入树（设计上前台只有终值，不参与 live 树，保持 P2 Stage F 边界）。

---

## 2. 现状核验（已逐文件查证，非 Explore 推断）

| 事实 | 锚点 | 含义 |
|---|---|---|
| `ChainContext` 只有 `chain_id/depth/max_depth`，**无 parent_id** | `src/harness/chain_context.rs:31-39` | 层级深度**已在**，仅缺父引用 |
| `BackgroundAgentTracker` 全进程单例、扁平 `HashMap<request_id,_>`，节点**不存 parent/depth/root** | `src/agents/background_tracker.rs:15-36` | 树骨架字段缺失，但 request_id 已是稳定 node_id |
| `ForwardingTraceSink` 持 `request_id` + tracker + inner sink，已翻译 4 类进度事件 | `src/agents/forwarding_trace_sink.rs:23-95` | live 事件外发的**天然注入点** |
| spawn 生成 `request_id=uuid`，持有 `parent_agent_id`/`parent_session_id`，完成时经 `GlobalBus` 广播 `AlephEvent::SubAgentCompleted` | `src/agents/subagent_tool/spawn.rs:65-170` | 跨 agents→gateway 的事件通道**已存在**，可复用 |
| 完成事件 `SubAgentResult{agent_id,child_session_id,...}` **无 depth/parent_id** | `src/agents/subagent_tool/spawn.rs:149-158` | 树元数据缺失 |
| gateway 订阅 `SubAgentCompleted` 驱动父 turn（announce 模式） | `src/gateway/subagent_announce.rs:47-68` | live 转发的**模板代码** |
| panel 仅有线性 trace 时间线（订阅 `run.agent_trace`）+ teams 任务 DAG，**无子智能体树** | `interfaces/webchat/src/platform/wide/views/agent_trace.rs:66-73`、`.../teams/plan_dag.rs` | 树状图本体待建 |
| `aleph_protocol` 是纯类型共享 crate，已被 WASM panel 引用 | `shared/protocol/src/lib.rs:31-50` | **单一 Rust 树重建源的落点** |

---

## 3. Gap Analysis（hermes ↔ Aleph 取优映射）

| 维度 | hermes | Aleph 现状 | 取优方案 |
|---|---|---|---|
| 节点身份 | `subagent_id+parent_id+depth` 预生成 | request_id 扁平 + depth(ChainContext) | 复用 request_id 为 node_id；parent_id 在 runtime 层穿线（**不碰 harness**） |
| 树结构 | 无中心管理器，事件携身份→客户端重建 | 无 | **采纳身份穿线**；但重建算法写 Rust 一份，原生+WASM 复用（超越 hermes 的 Py+TS 双写） |
| 生命周期 | 字符串状态 | 仅 `CompletedOutcome{Ok,Err}` 终态 | **Rust 超越**：typed `NodeLifecycle` 状态机，非法态不可表示 |
| 并发 | ThreadPoolExecutor(GIL) | Tokio task 真并行 + semaphore | 天然胜出，无改；不引 Rayon（非 CPU 密集，违 R3） |
| 进度外发 | 进度回调中继 + 批量 flush | 仅 push_progress(50 FIFO)，不外发 panel | 复用 GlobalBus 广播 + gateway forwarder → `run.subagent_tree` topic |
| rollup | totalTools/duration/descendant/active/hotness | 无 | 共享 `rollup()` 一次递归算（原生 RPC + WASM live 同源） |
| 可视化 | TUI overlay 全套 | 无 | 新建 Leptos 视图全平价 |

---

## 4. 关键架构决策：单一 Rust 树重建源

webchat 是 **Leptos/WASM Rust**，且已依赖 `aleph_protocol`。因此把**树类型 + 重建算法 + rollup 写在 `aleph_protocol::subagent_tree` 一处**：

- **原生侧**（aleph-server）：`subagent.tree` RPC 调 `build_tree()` 返回冷启动快照。
- **WASM 侧**（panel）：维护 `HashMap<node_id, SubagentNode>` 扁平表，live 事件增量更新后调**同一个** `build_tree()` 重渲染。

→ hermes 在 Python(后端聚合) + TypeScript(`subagentTree.ts` 前端重建) **各写一遍**；Aleph 一份 Rust 编两端。这同时是**性能超越**（serde 零拷贝、类型安全）**与熵减**（单一真源，杜绝双实现漂移）。

---

## 5. 数据模型（`shared/protocol/src/subagent_tree.rs`，新文件）

```rust
/// 节点生命周期 — typed 状态机（替代 hermes 字符串状态）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeLifecycle { Running, Completed, Failed, Cancelled, TimedOut }

/// 扁平节点（事件携带 / RPC 返回 / 客户端表项 共用）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubagentNode {
    pub node_id: String,              // = tracker request_id
    pub parent_id: Option<String>,    // None = 直挂 root session
    pub depth: u32,                   // ChainContext.depth（1 = root 直接子）
    pub root_session: String,         // 树归属（panel 按此过滤）
    pub task: String,
    pub model: Option<String>,
    pub lifecycle: NodeLifecycle,
    pub started_at_ms: u64,
    pub elapsed_ms: u64,
    pub tool_count: u32,
    pub last_tool: Option<String>,
    pub last_activity: Option<String>, // "tool_called"/"llm_thinking"/...
}

/// live 事件（身份穿线 — 客户端无状态重建的关键）
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubagentTreeEvent {
    Spawned   { node: SubagentNode },
    Progress  { node_id: String, root_session: String, step: usize,
                activity: String, tool_name: Option<String>, tool_count: u32 },
    Settled   { node_id: String, root_session: String, lifecycle: NodeLifecycle,
                duration_ms: u64, iterations: usize, tool_calls_made: usize, total_tokens: usize },
}

/// 组装后的树节点（含递归 rollup）
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeNode {
    pub node: SubagentNode,
    pub children: Vec<TreeNode>,
    pub rollup: Rollup,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Rollup {
    pub descendant_count: u32,  // 含自身子树规模
    pub active_count: u32,      // 子树内 Running 数
    pub total_tools: u32,       // 子树工具调用总和
    pub total_duration_ms: u64, // 子树耗时总和
    pub max_depth_from_here: u32,
    pub hotness: f32,           // total_tools / (total_duration_s)，热度着色用
}

/// 扁平 → 森林（按 parent_id 分组，孤儿/未知父挂为 root；children 按 depth,started_at 排序）
pub fn build_tree(flat: &[SubagentNode]) -> Vec<TreeNode>;
/// 递归聚合（build_tree 内部调用，单次 O(n)）
fn compute_rollup(node: &SubagentNode, children: &[TreeNode]) -> Rollup;
```

设计要点：
- **孤儿容忍**（hermes 模式）：parent_id 指向未知/已 TTL-prune 节点时挂为顶层，不丢节点。
- **状态机单调**：lifecycle 只能 Running → 终态，便于安全聚合。
- 事件 `Settled` 合并 Completed/Failed/Cancelled/TimedOut，由 `lifecycle` 区分（少一个事件类型，对齐 R7 少即是多）。

---

## 6. 后端连线（Phase 0–2）

### Phase 0 — 节点身份记录（零 harness 改动；架构核验后简化）
> **核验结论（2026-06-27）**：生产 `SubagentTool` 每个顶层 run 构造一次（`inner.rs:748`，`run_chain=ChainContext::new()` depth0、`parent_session_id=root`），子 harness 复用同一实例（`parent_view_for_children`）。叠加递归守卫（`types.rs:277` SubAgent 模式禁 `subagent` 工具），**后台子智能体树结构性为 2 层**（session 根 → depth-1 后台子智能体），与 hermes 默认 `MAX_DEPTH=1` 一致。
> → 因共享工具实例使 node_id 穿线无效，**不穿线 node_id**：`parent_id=None`（挂 session 根）、`depth=child_chain.depth`、`root_session=parent_session_id`。数据模型保留 `parent_id`/`depth` 字段令 `build_tree` 支持任意深度（未来若改 per-child 工具即可填充），是诚实的"结构就绪、不造无法行使的穿线"。

1. `BackgroundAgentTracker` 的 `RunningAgent`/`CompletedAgent` 增 `parent_id/depth/root_session/model/tool_count/last_tool/last_activity` 字段；`register()` 改收一个 `SpawnMeta` 结构（避免 too-many-args）；`push_progress` 顺带累加 `tool_count`、刷新 `last_tool`/`last_activity`。
2. `spawn_background`（`spawn.rs:65`）：`register` 传 `SpawnMeta{ parent_id: None, depth: child_chain.depth, root_session: self.parent_session_id, model }`。
3. 新增 `BackgroundAgentTracker::flat_nodes(root: Option<&str>) -> Vec<SubagentNode>`：running+completed 合并为 protocol 扁平节点（按 root 过滤）。

### Phase 1 — live 事件（复用 GlobalBus，gateway 轻量 forwarder）
4. 新增 `AlephEvent::SubAgentTreeUpdate(SubagentTreeEvent)`（`src/event/types.rs`）。
5. 发射点（agents 层，经 `GlobalBus::global().broadcast`，scope=root_session）：
   - **Spawned**：`spawn_background` register 后。
   - **Progress**：`ForwardingTraceSink::on_trace` 翻译进度时一并发（sink 增 `parent_id/depth/root_session` + 一个 `GlobalBus` 句柄；保持 R1/P4，不依赖 gateway 具体类型）。
   - **Settled**：`spawn_background` 完成处（与现有 `SubAgentCompleted` 并列发，互不取代——announce 仍驱动父 turn，tree 只刷 UI）。
6. gateway 新增轻量订阅者 `subagent_tree_relay.rs`（**仿 `subagent_announce.rs` 但不驱动 turn**）：订阅 `SubAgentTreeUpdate` → 经 `GatewayEventBus` 以 topic `run.subagent_tree` 投递到 root_session 流。**R4/R10 合规**：纯转发，零推理。

### Phase 2 — 树快照 RPC
7. `BackgroundAgentTracker::flat_nodes(root_session: Option<&str>) -> Vec<SubagentNode>`（按 root 过滤；running+completed 合并）。
8. gateway handler `subagent.tree`（只读 RPC，`src/gateway/handlers/`）：调 `flat_nodes` → `build_tree` → 返回 `Vec<TreeNode>`。panel 冷启动拉一次，之后靠 live 增量。

---

## 7. 前端树状图（Phase 3，丰富版）

新建 `interfaces/webchat/src/platform/wide/views/subagent_tree/`（按职责拆多文件，符合 P2 高内聚）：
- `mod.rs`：视图组件，路由 `/dashboard/subagents`。
- `state.rs`：`HashMap<node_id, SubagentNode>` 信号；冷启动 `subagent.tree` RPC + 订阅 `run.subagent_tree` 增量；每次变更调 `aleph_protocol::subagent_tree::build_tree` 重算。
- `render.rs`：递归树渲染——展开/折叠、缩进引导线、状态字形（● 运行 / ✓ 完成 / ✗ 失败 / ⌛ 超时 / ⊘ 取消）。
- `heatmap.rs`：按 `rollup.hotness` 冷→热着色；`sparkline.rs`：`▁▂▃▄▅▆▇█` 深度分布。
- `controls.rs`：排序（深度/工具数/耗时/状态）+ 过滤（全部/运行/失败/叶子）+ 摘要行 `d{maxDepth} · {N} agents · {tools} tools · {duration} · ⚡{active}`。

i18n key 走现有 `t!`/`use_i18n` 体系；live 订阅复用 `agent_trace.rs:66` 同款 `state.subscribe_events`。

---

## 8. Phase 4 — 熵减（诚实声明）

本重构以**新增 + 单源**为主，**不存在大量旧死代码可删**。熵减承诺具体为：
1. **杜绝平行实现**：树重建只在 `aleph_protocol` 一处（不在 TS/JS 另写）——结构性熵减。
2. **清理自造孤儿**：若 `register()` 签名改造后旧 3 参调用点全部迁移，删除任何因此空置的中间适配；`SubAgentResult.tools_called`（`spawn.rs:156` 恒 `Vec::new()`）若被 tree 的 tool_count 取代且确认无消费者，则就地移除（**先 grep 验消费者再删**，遵记忆 `aleph-audit-verify-findings`）。
3. 不删任何**先于本次重构存在**的死代码（遵全局 CLAUDE.md §3）。

---

## 9. 红线合规

- **R10（薄 harness）**：`src/harness/` **零改动**——depth 复用现有 `ChainContext`，身份穿线全在 `src/agents`/runtime 层。树重建/rollup 在 protocol crate，非认知逻辑。
- **R4 / gateway 边界**：`subagent_tree_relay` 纯 I/O 转发，零推理；RPC 只读。
- **R7（LLM 主权）**：树是**可观测性**产物，不参与任何路由/意图/完成度判断；父 LLM 仍走 `subagent` 工具的 list/check_status（不变）。
- **R3 / P4（轻量 + 依赖倒置）**：不引新依赖（serde/leptos 已在）；agents 层经 `GlobalBus`（已有抽象）发事件，不反向依赖 gateway 具体类型。

---

## 10. 测试

- **protocol**：`build_tree` 单测——多层级、孤儿挂顶、children 排序、rollup 递归数值、hotness 边界（duration=0 不除零）。
- **tracker**：`flat_nodes` 按 root 过滤；`register` 携带元数据往返；`push_progress` 累加 tool_count。
- **forwarding_sink**：Progress 事件含正确 `node_id/parent_id/depth`。
- **gateway relay**：`SubAgentTreeUpdate` → `run.subagent_tree` topic 转发（不驱动 turn）。
- 前端：`build_tree` 既已在 protocol 测，WASM 侧只测 state 增量 reduce（可选）。

> 资源约束：按指令**完成后不跑 cargo check，直接提交**；测试随代码落地但不在本会话执行。

---

## 11. 实施顺序与分支

worktree 隔离分支（如 `feat-subagent-tree`），严禁触 main。提交粒度：Phase 0–2（后端）一组、Phase 3（前端）一组、Phase 4（清理）随附。Scan→Plan→Implement→Commit。
