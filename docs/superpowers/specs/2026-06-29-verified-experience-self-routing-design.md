# 设计规格：Verified-Experience Self-Routing (VESR)

> 状态：Design Spec（已纳入对抗性评审，待用户终审 → 交付 planning step）。本文不含实现步骤（那是 plan 的职责），只定义"做什么、为什么、连在哪、如何验收"。
> 命名约定：散文中文，代码/标识符/文件路径英文。所有 file:line 锚点均为代码核验后的真实坐标。
>
> **⚠️ 待用户确认的 v1 scope（唯一一处）**：§5.5 给 `list_models` 增加**全局 per-model 原始聚合**字段，本质是 Q3 里你选择**延后**的"聚合排行榜"（你的 v1 选择＝"纯 embedding kNN 邻居召回，聚合后续叠加"）。本 spec 默认**把 §5.5 视为 v1.1（不在 v1 实现）**；v1 = 3 个核心单元（store + observer + recall）的纯 kNN 召回路径。若你想让全局聚合先验进 v1，明确说，我把 §5.5 拉回并同步 §4 / §6 / §9-N3 / §11-O10·O11 的交叉引用。

---

## 1. 背景与问题

论文 **2606.22902《Agent-as-a-Router》** 的核心结论：LLM 模型路由的瓶颈是**信息缺失（information deficit），而非推理能力**——当把"每类任务上各模型的已验证表现"喂给做选择的 LLM 时，路由质量提升 **+15.3%**。换句话说，会选模型的 LLM 不缺脑子，缺的是"我以前在类似任务上用某模型的真实结果如何"这条经验。

Aleph 现状已经具备**让 LLM 自主选模型**的全部机件：

- `select_model` 工具（`src/builtin_tools/select_model.rs`）让 LLM 在会话内改用某模型；
- `list_models` 工具（`src/builtin_tools/list_models.rs`）已是 R7 对齐的"纯数据发现面"，提供能力表 + 价格表；
- 子代理 spawn 时父 LLM 经 `SpawnRequest.model`（`src/agents/subagent_spawner/mod.rs:98-110`）指定子代理模型。

**唯一缺的就是论文指出的那条信息**：各模型在"类似过往任务"上的已验证结果。VESR 只补这条信息，**决策权 100% 留在 LLM**。

行业定位（一句话）：

- **RouteLLM**：训练一个学习型 router（分类器/矩阵分解）在强/弱模型间做确定性路由——Aleph 拒绝（违 R7/R10：把决策搬出模型）。
- **semantic-router**：基于 embedding 把话语路由到**预定义离散路由**并确定性选择——Aleph 复用其 embedding 召回思路，但**不要离散标签、不要确定性 pick**（标签化=意图分类红线 R7/R10）。
- **litellm / bifrost**：运营级网关（负载均衡、failover、跨 provider 成本路由）——这是 Aleph 已有的**运营 route_policy 层**，与 VESR **正交且不动**。VESR 选的是"推理质量意义上的模型"，route_policy 选的是"provider 容灾/负载"。

VESR 的本质一句话：**把那些系统塞进独立 router 模块的"已验证经验"，作为信息喂给 Aleph 已有的、LLM 主权的自路由器——但不引入那个模块。**

---

## 2. 设计目标与非目标

### 目标（v1）

1. 在**会话 run-to-run** 维度：run 开始时把"类似过往任务上各模型的真实结果"召回注入；`select_model` 写会话偏好，**下一个 run 生效**。
2. 在**子代理 spawn-time** 维度：父 LLM 从经验中选子代理模型（经 `SpawnRequest.model`）。
3. 反馈信号是**零成本、零判断**的结构化观测：从 trace 直读/派生（terminate_reason 原始变体、iterations、tool 错误数、token 成本、duration）。
4. 召回粒度 = **纯 embedding kNN 邻居（retrieval，非 classification）**，复用既有 sqlite-vec；**无离散任务类型标签**，任务类型由 LLM 在上下文中自行推断。

### 非目标（明确 YAGNI）

- ❌ **不建 router 模块 / 分类器 / 确定性 policy / bandit**。决策权属 LLM。
- ❌ **不做 mid-run 升档**（运行中换模型）——`select_model` 经核验是 run 构造时一次性冻结（见 §7 与核验事实 `model_binding_timing_v1_verification`），mid-run = **v2**。
- ❌ **不做 synthetic probing prior / 冷启动探测**——冷启动 = 今天的盲选行为（见 §8 D1）。
- ❌ **不引入 epsilon-greedy / 成本阈值 / 最小样本门限旋钮**——这些都是判断，由 LLM 在上下文内权衡。
- ❌ **不动运营 route_policy 层**（cost/usage/latency/failover）——正交。
- ❌ **v1 不建 LLM-judge / user-signal 反馈覆盖层，且不预留接口/seam/placeholder**——零现有消费者的抽象即 R10 明令删除的反模式（dissolution 期累计删 ~5,200 行此类口子）。**只在出现第二个真实信号源时才新增抽象**。
- ❌ **不引入离散 task-type 标签或意图分类**。
- ❌ **不在代码内对 outcome 做跨任务难度归一化/打分**（见 §8 D7）。

---

## 3. 宪法对齐

| 红线 | VESR 如何严格遵守 |
|------|------------------|
| **R7 LLM 主权** | 系统**只提供原始信息**（已验证经验的逐字字段），不替模型做任何路由判断。无规则引擎、无分类器、无 success bool、无 score、无 argmax、无跨模型 ranking、无 `best_for_task_types` 标签。`select_model` 决策逻辑保持纯 passthrough（`select_model.rs:64-102` 零过滤零打分）。召回用 embedding kNN（retrieval）而非离散标签（classification），避免"意图分类"红线。 |
| **R10 薄 Harness / 笨循环** | `OutcomeObserver` 作为 **`TraceSink` 装饰器**实现，完全在 `src/harness/` **之外**（`src/routing/`），不进 12 文件/~4900 行预算（核验事实 `trace_sink_observer_outside_harness`）。**模型归因走 sink 构造期捕获（§7），`src/harness/trace.rs` 字面零改动**。`RoutingRecall` 在 `src/orchestrator/harness_bridge/prompt_build.rs` 的 run-start seam 注入，不进 think 循环。循环 5 个"不"全不触碰：不分类意图、不做工具过滤、不判完成度、不做内容审查、不选错误恢复策略。 |
| **R3 核心轻量化** | 不引第三方向量库（复用 sqlite-vec）、不引第二 embedder（复用 `Arc<dyn EmbeddingProvider>`，`embedding_provider.rs:10-27`）。新增机件是小而内聚的 `src/routing/` 几个文件 + 一个 memory sibling store，非沉重依赖；无并行模块帝国（见 §5 落位）。 |
| **R8 工具即一切** | 召回经 `list_models` 工具面 + run-start 上下文注入呈现给 LLM；选择经 `select_model` / `subagent` 工具完成。对话即配置，无新协议。 |
| **A2 错误压缩≠错误恢复** | OutcomeObserver 把工具/Provider 错误**压缩成结构化 outcome 数据**（错误计数、原始 terminate_reason 变体）喂回模型供其下一 run 自行权衡——**采纳**；绝不在 harness 里做确定性"错误恢复策略选择 / 重试矩阵"——**仍禁**。让模型看见并据此选模型 = 要；让 harness 替模型挑模型 = 不要。 |

### 观测 ≠ 判断（本设计的拱顶石纪律）

> **硬规则**：OutcomeObserver **逐字搬运 trace 里已有的结构化事实**到存储列（计数、布尔判别式、枚举变体、token 数），**绝不打分、绝不归类、绝不评价、绝不计算 success bool 或 composite quality**。
>
> 一旦观测器把 `terminate_reason` 塌缩成"成功/失败"布尔，或把 `{低 iterations, 低 cost, 低错误率}` 合成一个"质量分"，路由裁决就从 LLM 搬进了确定性代码——正是 R7（LLM 主权）+ R10 五-不 #4/#5 留给模型的部分。"结构化 outcome → 这模型好不好" **本身就是隐式判断**。
>
> "这次 run 好不好" 由 LLM 在下一 run 看到**原始** outcome 后自己判断。store.record 的职责是写列，不是解读。

---

## 4. 架构映射表（论文 → Aleph）

| 论文角色 | 职责 | Aleph 对应 | 关键说明 |
|---------|------|-----------|---------|
| **Orchestrator**（选模型的 router） | 给任务挑模型 | **❌ 不作为模块移植** → 落在 **LLM 的上下文推理**（`select_model` / `SpawnRequest.model`） | 移植一个 Orchestrator 模块=引入确定性路由决策者=**直接违 R7（LLM 主权）+ R10（循环内/旁不放分类器与决策）**。"编排"留在 LLM 脑中，由已验证经验喂养。 |
| **Verifier**（判任务结果） | 评估 outcome | **零判断结构化观测**：`FlowOutcome`（`src/orchestrator/dispatch.rs:63-112`）+ `TerminateReason`（14 变体，`dispatch.rs:120-190`，原样持久化）+ `tool_timeline` 错误计数 | v1 用**逐字结构化派生**而非 LLM-judge。**不建也不预留** LLM-judge 覆盖层（零消费者抽象违 R10）。 |
| **Memory**（存已验证经验） | 持久化 task→model→raw outcome | **`RoutingExperienceStore`**：`SqliteMemoryBackend` 上的 sibling store + sqlite-vec | 复用 embedder + 向量基建，但**不挂 MemoryFact 语义**（见 §5.1 核验纠正）。 |
| **ToolLayer**（暴露选择） | 给 LLM 选择面 | `list_models`（数据面）+ `select_model`（写偏好）+ `subagent` 工具 `model` 参数（`loop_tool.rs:102-104`） | 全部已存在，仅给 `list_models` 增**中性原始聚合**字段（非 verdict）。 |

> **为什么 Orchestrator 不移植**——这是全设计的拱顶石。论文的 Orchestrator 是个会做决定的模块；Aleph 宪法 R7/R10 明令禁止任何"替模型做路由判断"的确定性单元出现在循环内或循环旁。VESR 的全部价值在于**证明：不需要那个模块，只需要把它本该消费的信息直接喂给已经会自路由的 LLM。**

---

## 5. 组件设计（3 个小单元 + 1 个存储原语）

**落位纪律（R3/P6，避免模块帝国）**：`src/routing/` **已存在**（`select_model` 的测试已 import `crate::routing::session_key`）。VESR 在其下平铺新增文件，**不另起 `experience/` 子目录**。唯一落在 `src/memory/` 的是一个**存储原语 sibling**（镜像 `DreamReportStore` 范式：`SqliteMemoryBackend` 上的方法，自有表，与 NoteStore/MemoryFact 零耦合）。

```
src/routing/                       # 已存在
├── mod.rs                # 装配 + RoutingAttribution 每-run 关联句柄
├── experience_store.rs   # RoutingExperienceStore facade + RoutingOutcome/RoutingNeighbor 类型
├── observer.rs           # OutcomeObserver（TraceSink 装饰器）
└── recall.rs             # RoutingRecall（run-start 上下文提供者）
src/memory/store/sqlite/
└── routing_experience.rs # 存储原语：impl on SqliteMemoryBackend（镜像 dream_reports.rs:1-62）
```

### 5.1 RoutingExperienceStore — 经验存储

**职责**：`record(task_key, model_id, provider_id, raw_outcome)` / `recall(task_key, k)`。

**核验纠正（NUANCED，必读）**：核验事实 `routing_experience_store_memory_layer_reusability` 把"扩展 memory 层"判为 **HIGH COUPLING RISK**。**不要**把路由经验做成 `NoteType` 变体或 `MemoryFact` 子类——否则会继承 `is_valid`/`decay_invalidated_at`/`invalidation_reason`（用户记忆的软删/回收站语义，路由经验绝不应被 memory daemon 过期或软删）、`MemoryCategory`/`NoteType`（用户面分类）、`namespace`（owner/guest/shared 访问控制）、以及 `rerank_score(cosine, confidence, severity)` 的打分公式（混入 LLM 蒸馏质量，对任务-结果排序不适用，`retrieval.rs:36-80`）。

**正确做法**：照搬 `DreamReportStore` 模式（`dream_reports.rs:1-62`）——**不是 trait，而是 `SqliteMemoryBackend` 上的方法**，自有表，与 NoteStore/MemoryFact 零耦合。

存储原语（`src/memory/store/sqlite/routing_experience.rs`，schema 走 `schema/ddl.rs` 的 DDL-常量模式）：

```rust
// 自有 DDL，不碰 MemoryFact/NoteType。所有列都是逐字原始事实——无 success/score/rank 列。
// ROUTING_EXPERIENCE_DDL: id PK, agent_id, model_id, provider_id,
//   terminate_reason TEXT(原始变体判别式+内嵌字段), iterations INT, tool_calls INT,
//   tool_error_count INT, tool_call_total INT,
//   tok_input/tok_output/tok_cache_read/tok_cache_creation/tok_reasoning INT,
//   estimated_cost REAL?, duration_ms INT, context_tokens INT, context_window INT,
//   created_at INT
//   + 维度专属向量表 routing_exp_vec_768 / _1024 / _1536（镜像 notes_vec_*）
impl SqliteMemoryBackend {
    pub(crate) async fn record_routing_experience(/* 逐字写列，零解读 */) -> Result<(), AlephError>;

    // KNN：复用 vec0 MATCH（vec.rs:36-55 的 embedding_to_blob，
    // store_impl.rs:905-996 的 vector_search 模式），按 agent_id 隔离（见 §8 D9），
    // 返回 (raw columns..., L2_distance)
    pub(crate) async fn recall_routing_experience(
        &self, task_emb: &[f32], dim: u32, agent_id: &str, k: usize,
    ) -> Result<Vec<RoutingNeighbor>, AlephError>;
}
```

领域 facade（`src/routing/experience_store.rs`）复用 `Arc<dyn EmbeddingProvider>`（`embedding_provider.rs:10-27`，模型无关、可注入、**全程同一实例不重复实例化**）、`embedding_to_blob`、`notes_vec_table_for_dim` 风格维度映射、`vector_search` 的 vec0 KNN、`1.0/(1.0+distance)` 的 L2→相似度换算。`recall` 返回的每条邻居**自带 `distance` 与 `created_at` 与 `n_runs` 上下文**，供 LLM 折扣远邻、折扣陈旧、折扣小样本（见 §8 D5/D7、§11）。

### 5.2 RoutingOutcome — 零判断反馈面（三类信号显式分层）

核验事实 `outcome_observer_feedback_surface` 把信号分三类，spec 必须显式标注，防止读到永远的 0/None：

```rust
pub struct RoutingOutcome {
    // (1) FlowOutcome 直读字段（安全，逐字搬运）
    pub iterations: u32,
    pub tool_calls_made: u32,
    pub terminate_reason: String,   // TerminateReason 原始判别式 + 内嵌字段，例如
                                    // "VerifierVeto{vetos}" / "ConsecutiveFailureCap{consecutive}"
                                    // / "HitMaxIterations{used}" —— 原样落库，绝不塌缩成成功/失败
    pub token_breakdown: TokenBreakdown, // input/output/cache_read/cache_creation/reasoning
    pub estimated_cost: Option<f64>,
    pub duration_ms: u64,
    pub context_tokens: u32,
    pub context_window: u32,
    // (2) 派生字段（在 tool_timeline 上计算，仍是原始计数，非判断）
    pub tool_error_count: u32,   // = tool_timeline.iter().filter(|t| !t.success).count()
    pub tool_call_total: u32,    // = tool_timeline.len()
}
// (3) 缺失信号（显式不读，禁止伪造）：
//   - user_re_steer：trace 无显式 user-steering 事件（核验 trace.rs:24-125 无 UserSteer/UserInterrupt 变体）；
//     仅 TerminateReason::Cancelled 或 prompt 重放里的 INTERRUPTION_NOTE 可见 → v1 不纳入、不实现读取
//   - consecutive_errors：LoopTraceTurnMetrics 字段恒为 0（think.rs:~850 硬编码，vestigial） → 禁止读
```

> **设计纪律**：任何"用户 re-steer 次数"或"consecutive_errors 准确值"在 v1 **不可得**——读它们只会得到静默 0/None（虚假现实）。spec 明确禁止实现者读取这两者。若要纳入 user re-steer 须新增 trace 事件，与"零成本观测"冲突且改 harness/gateway 管线——**v1 不做、不预留**（见 §2 非目标）。
>
> **此结构里没有也不会有 `success: bool` 或 `quality_score: f32`。** 所有"好坏"判断在 LLM 推理时发生。

### 5.3 OutcomeObserver — trace_sink seam 上的纯观测器

**职责**：作为 `TraceSink` 装饰器，捕获 `SessionCompleted`（取 outcome），逐字派生 `RoutingOutcome`，**fire-and-forget** 写入 store。**模型归因走 §7 的 sink 构造期捕获——observer 不读 `ProviderUsage` 做模型判断。**

**位置**：`src/routing/observer.rs`，实现 `harness::TraceSink`（公开再导出，`harness/mod.rs:18`），装配进 `inner.rs:640-690` 的 sink 组合栈——**完全在 harness 之外**（核验确认 `ForwardingTraceSink`/`UnattendedRedactingSink`/`AgentTraceEmitSink` 装饰器范式均居 `src/agents`、`src/gateway`，非 `src/harness`）。

```rust
pub struct OutcomeObserver {
    inner: Arc<dyn TraceSink>,                 // 装饰器：观测后原样转发
    store: Arc<RoutingExperienceStore>,
    attribution: Arc<RoutingAttribution>,      // 每-run 句柄：recall 写 task_emb，observer 读
    model_id: String,                          // §7：构造期注入的冻结主模型（不从 trace 读）
    provider_id: String,                       // §7：构造期注入
}
impl TraceSink for OutcomeObserver {
    fn on_trace(&self, event: &LoopTraceEvent) {
        if let LoopTraceEvent::SessionCompleted {
            iterations, tool_timeline, terminate_reason, token_breakdown,
            duration_ms, .. } = event {
            // 逐字派生 RoutingOutcome（零解读）；读 attribution.task_emb；
            // tokio::spawn 写 store.record(task_emb, self.model_id, self.provider_id, outcome)
        }
        self.inner.on_trace(event);  // 必须非阻塞 + 原样转发（trace_sink.rs:12-25）
    }
}
```

**两个约束（来自核验）**：
1. `on_trace` **MUST NOT block**（`trace_sink.rs:12-25`）→ `store.record` 走 `tokio::spawn` 火-忘，镜像 `emit_delegation_primitives`（`a2a/sub_agent.rs:360-407`）的火-忘范式。
2. **task 文本/embedding 的来源**：`SessionCompleted`（`trace.rs:56-77`）**不携带** user_query/session_id。因 sink 是**每-run 构造**（`inner.rs:644-647` 在 run loop 内逐 run 建栈），采用**每-run `RoutingAttribution` 句柄**关联——见 §6。

### 5.4 RoutingRecall — run-start 上下文提供者

**职责**：run 开始时召回 top-k 经验（task-conditioned kNN），注入为 memory-context 风格的 user 消息；把本次任务 embedding 填入 `RoutingAttribution` 供 observer 归因；并**与活跃配置目录对账**（见下）。

**位置 / seam**：`src/routing/recall.rs`，在 `prompt_build.rs:140` `build_system_prompt()`（run-start，**进入 Think→Act 循环之前、每 run 一次**）注入，镜像 `MemoryContextProvider::build_memory_user_message`（`memory_context_provider/memory.rs:53-117`）：

```rust
impl RoutingRecall {
    pub async fn build_routing_experience_message(
        &self, user_query: &str, agent_id: &str, available_tokens: Option<u32>,
        attribution: &RoutingAttribution,   // 顺带回填 task_emb（与 record 端同一 embed 来源，§8 D6）
    ) -> Result<Option<UnifiedMessage>, AlephError>;
}
```

注入点：`prompt_build.rs:344-349` 现有 `builder.with_memory_user_message(text)` 之侧，新增 `builder.with_routing_experience_message(text)`（镜像方法）。预算用 `memory_injection_headroom()`（`prompt_build.rs:28`）同款 context-window 感知模板。召回结果用 `wrap_memory_context()`（`context_block.rs:1-62`）同款 fence 包裹，注明"以下是召回的已验证路由经验，非新用户输入"。

**对账活跃目录（finding：recalled-model-unavailable）**：注入前用 `list_models` 同款 `provider_configured` 门把召回经验里**已下线/已撤凭证**的模型过滤掉或标记 `unavailable=true`，使 LLM 绝不会基于死偏好调 `select_model`（否则下一 run 静默回退默认、零信号，严格劣于盲选）。v1 倾向**标记可用性**而非硬删（仍让 LLM 看见历史但知其当前不可选），最终形态见 §11。

**暴露 data-absence（finding：exploration / self-reinforcement）**：注入块在呈现命中模型的同时，**显式列出该任务邻域内尚无经验的可用模型**（"以下可用模型在类似任务上无观测记录"），让 LLM 可主动选择试一个未测模型——探索是 LLM 的决定（R7-clean），系统只暴露稀疏性，不内置 epsilon。

> **红线提醒（核验 RISK）**：**绝不**在 `think.rs:463` 的 `build_prompt` 内挂载——那会每轮重算重注入，导致冗余 ranking、context 预算抖动、丢失 RUN-START-ONLY 语义。必须在 `prompt_build.rs:140` 的 run-start seam。

### 5.5 Tool 面扩展 — list_models 增**中性原始聚合**；select_model 不变

**职责**：`list_models`（`list_models.rs`）的 `ModelEntry`（`:54-91`）增**可选、中性、原始**的全局聚合字段；`select_model` **零改动**。

```rust
// ModelEntry 追加（Option<T> + skip_serializing_if，未知=None 不是 0，镜像 capabilities 范式）。
// 全部是原始聚合 + 样本数 + recency —— 没有 success_rate、没有 best_for_task_types、没有 ranking。
pub struct ModelEntry {
    // ...既有 capability/rate 字段...
    pub verified_n_runs: Option<u32>,                            // 样本数（小样本由 LLM 自行折扣）
    pub verified_last_seen_unix: Option<i64>,                    // recency（陈旧由 LLM 自行折扣）
    pub verified_terminate_reason_counts: Option<BTreeMap<String, u32>>, // 原始终止原因分布
    pub verified_median_iterations: Option<f32>,
    pub verified_median_cost_per_run: Option<f64>,
    pub verified_tool_error_rate: Option<f32>,                   // 原始聚合 = Σerror/Σtool，非 verdict
}
```

**关键设计区分（task-conditioned vs global）**：
- `list_models` 暴露**全局 per-model 原始聚合**（便宜、随时可得，因 list_models 是 LLM 任意时刻可调的工具，**不知当前"任务"**）。
- **task-conditioned kNN 邻居经验**由 §5.4 `RoutingRecall` 在 run-start 注入（它有当前任务文本）。
- LLM 同时看到两者：run-start 注入的"类似任务上各模型的真实结果（含 distance/recency/sample）" + list_models 的"各模型全局原始战绩"，**自行综合、自行归一化、自行权衡质量 vs 成本**。

**R7/R8 保持（核验 RISK：避免在 enrich 嵌入路由逻辑）**：`enrich()`（`:146-173`）原为纯/无 I/O；新增**异步统计 pass** 查 store 填可选字段——这是**数据富化非自动路由**（`capabilities.rs:11-13` 的"data, not routing"原则）。**绝不**在 enrich 里嵌入"哪个最好"的判断、success 定义或跨模型排序。`select_model`（`:64-102`）保持纯 passthrough，零过滤零能力检查（diff 必须为空，见 §9 N2）。

---

## 6. 数据流 C-A-F（Capture → Attribute → Feed）

**每-run 关联句柄**（解决 §5.3 的 task-emb 关联，全在 gateway run loop，harness 之外）：

```rust
// src/routing/mod.rs，run_loop 内每 run 构造一个
pub struct RoutingAttribution {
    pub session_id: SessionId,
    pub task_emb: OnceCell<Vec<f32>>,    // RoutingRecall 在 run-start 写入；observer 在完成时读
}
```

### C — Capture（构造时 + 运行中）
- **模型**：本 run 的冻结主模型在 sink 栈构造前已解析（§7），构造 `OutcomeObserver` 时把 `(model_id, provider_id)` 注入其自有状态——**不依赖任何 trace 事件携带模型**。
- **outcome**：run 结束，harness 发出 `SessionCompleted`（带 iterations/tool_timeline/terminate_reason/token_breakdown/duration_ms）。

### A — Attribute（完成时）
- `OutcomeObserver.on_trace(SessionCompleted)`：逐字派生 `RoutingOutcome`（直读 + tool_timeline 派生错误计数），读 `attribution.task_emb`，`tokio::spawn → store.record(task_emb, self.model_id, self.provider_id, outcome)`。原样转发事件给 `inner`。
- **每-run 自归因（finding：multi-agent attribution）**：每个 run（无论父/子）有**自己的** sink 栈、自己的 `RoutingAttribution`、自己的冻结模型。父 run 记到父冻结模型 + 父 task_emb；子 run 经其自身 `ForwardingTraceSink` 链上的 OutcomeObserver 记到子冻结模型 + 子 task_emb（`forwarding_trace_sink.rs:25-51, 91-105` 观测 SessionCompleted 的范式可复用）。**父记录绝不吸收子记录、子指标绝不写进父模型的列。** 已知残留：delegation-heavy 的父 run，其 cost/iterations 原始值天然含等待子代理的开销——v1 原样落库（不归一化），由 LLM 在上下文内辨识，见 §11。与既有 `emit_delegation_primitives → RawMemory(Delegation)`（`sub_agent.rs:360-407`，`types.rs:96-108`）**正交不重复**：delegation memory 存"任务+结果摘要"供长期记忆蒸馏经验，routing experience 存"模型+原始 outcome"供路由召回，两条链各司其职。

### F — Feed（下一 run / spawn 时）
**Seam 1 — 会话 run-to-run**：
1. 新 run，`prompt_build.rs:140` → `RoutingRecall.build_routing_experience_message(user_query, agent_id, headroom, attribution)`：embed user_query（同时回填 `attribution.task_emb`），`store.recall(task_emb, agent_id, k)`，对账活跃目录，包 fence 注入（含 distance/recency/sample/可用性/data-absence）。
2. LLM 看到"类似任务上各模型真实结果" + list_models 全局战绩 → 若决定换模型，调 `select_model` → `set_session_model`（`session_model_handle.rs:37-42`）。
3. 经 `model_binding_timing_v1_verification` 核验：模型在 run 构造时（`runner_impl.rs:85-105`）**一次性读取冻结**，故 select_model **下一个 run 生效**（v1 范围正确；mid-run = v2）。

**Seam 2 — 子代理 spawn-time**：
1. 父 run 上下文已含 run-start 注入的经验 + list_models 全局 per-model 原始战绩。
2. 父 LLM 经 `agent_info(agent_id)`（`agent_manage/info.rs`，含 model_hint）+ `agent_catalog` 概览（`agent_catalog.rs:12-54`）+ 注入的经验，决定子代理模型。
3. 经 `subagent` 工具 `model` 参数（`loop_tool.rs:102-104`）传入，精度链：**explicit `model` → `agent_def.model_hint` → native default**（`subagent_spawner/mod.rs:297-308`）。
4. 子 run 完成 → 子 run 自身的 OutcomeObserver 记录子 run 经验（归到子冻结模型，递归闭环）。

---

## 7. model_id 归因来源：sink 构造期捕获（harness 字面零改动）

核验事实 `model_binding_timing_v1_verification` 确认：模型在 **run 构造时一次性解析冻结**（`runner_impl.rs:85-105`，三级优先级 `get_session_model` → `model_hint` → `BrainRef`），包进 immutable `Arc<LLM>` 注入 `HarnessDeps`，整个 Think→Act 循环复用同一实例（`think.rs` 每轮 `self.deps.llm`），`select_model` 下一 run 生效。

**关键推论**：本 run 的模型在 gateway 构造 sink 栈之前（`inner.rs:640-690`，harness 之外）**已经解析就绪**。因此 OutcomeObserver **不需要从 trace 事件读 model**——在 sink 构造时把已解析的 `(model_id, provider_id)` 作为 observer 自有状态传入即可。

这同时干净地解决两个问题：

1. **R10 字面零改动**：核验事实 `trace_events_missing_model_attribution` 确认 `LoopTraceEvent::ProviderUsage`（`trace.rs:98-105`）与 `SessionCompleted`（`trace.rs:56-77`）均无 model_id 字段、且定义在 harness 内。sink 构造期捕获使归因**不依赖修改这两个变体**——`src/harness/trace.rs` 不动一字，12 文件/~4900 行预算零触碰。
2. **O2 run 内多模型污染天然消解**：若工具内部有不同模型的 LLM 调用，其 `ProviderUsage` 与本归因无关——我们只归因 harness brain 的**冻结主模型**（恰是 `runner_impl` 解析的那个），从不靠 `ProviderUsage` 反推模型。

**回退选项（仅当 plan 核查发现 sink 构造点确实拿不到已解析模型时）**：在 `ProviderUsage` 变体补 `model_id/provider_id` 两字段，在 `metering.rs:72-79` 构造处传 `req.model.clone()`（`adapter.rs:67` 的 `RequestPayload.model` 在 `:43` 可读，且 `ModelOverrideProvider` 于 `model_override_provider.rs:47` 已戳入、在 `orchestrator_impl.rs:111` 先于 MeteringProvider 包裹，故 emit 时模型已就绪）+ `self.inner.name()`（`metering.rs:48` 已取）。这是纯数据脚手架（过 R10"加代码前必答 3 问"：①脚手架非认知=纯数据搬运 ②模型升级仍需=让 trace 自描述是永久契约 ③有真实消费者=OutcomeObserver + 任何 billing/可观测下游）。**但若走此路，必须在 PR 里停止声称"harness 零改动"，并以三问显式论证那两字段。**

> **首选 sink 构造期捕获。** 仅在核验确证不可行时退到补字段方案；二者择一由 plan 拍板，本 spec 锁定"优先零改动"的方向。

---

## 8. 关键设计决定（v1 锁定）

### D1. 冷启动：无 synthetic prior
空 store → `recall` 返回空 → run-start 不注入经验，list_models 统计字段全 `None`（未知非零）→ **行为与今天的盲选完全一致**，随经验累积自然变好。**拒绝**合成探测先验：①合成先验=捏造判断，违背"观测非判断"；②探测有真实成本。冷启动不退化、不探测，是有意的 graceful baseline。

### D2. 成本即数据（cost-as-data）
`estimated_cost` / `token_breakdown` 是 `RoutingOutcome` 字段，经 recall 与 list_models 原样呈现；**LLM 自行权衡质量 vs 成本**。**无 epsilon 旋钮、无成本阈值**——任何确定性成本门=policy=违 R7。

### D3. 运营 route_policy 层不动
既有运营 `route_policy`（cost/usage/latency/failover 的 provider 路由，对应 litellm/bifrost 定位）**完全正交、零改动**。VESR 选"推理质量意义的模型"，route_policy 选"provider 容灾/负载"，二者组合不冲突。

### D4. 召回粒度 = 纯 embedding kNN，retrieval 而非 classification
任务相似性纯靠 `Arc<dyn EmbeddingProvider>` 的向量近邻，**绝无离散 task-type 标签**（标签化=意图分类，违 R7/R10）。任务类型由 LLM 在上下文中自行推断。recall 结果附 `distance`，让 LLM 自行折扣远邻。

### D5. OutcomeObserver 纯逐字观测，存原始变体非派生裁决
持久化的 outcome **是原始 `TerminateReason` 变体 + 原始计数/token + 原始 tool 错误数**，**绝无** `success: bool`、绝无 `quality_score`、绝无 argmax、绝无任何 store 内的跨模型 ranking。每条召回/聚合都**附样本数 n 与 recency**，让 LLM 在上下文内折扣小样本与陈旧数据——**不在代码内 bake 最小样本门限**（那是 policy 旋钮/判断）。这是 §3"观测 ≠ 判断"纪律的存储层落地。

### D6. task-key 文本：本 run 的 user_query，record/recall 严格对称
embedding key = **本 run 触发的 user_query**（与 memory recall 同款文本，`prompt_build.rs` 已有）。**record 端与 recall 端必须从同一文本派生 embedding，否则 kNN 失真。** 已知 multi-run 漂移（写入 §11）：形如"ok now do X"的跟进消息 embedding 近空，会召回不相关邻居——v1 接受此漂移、不在 v1 引入"运行任务摘要"作 key（避免提前抽象），作为开放问题留待数据验证。

### D7. 不在代码内做跨任务难度归一化
难任务天然多 iterations/高 cost，与模型无关。**绝不**在代码侧计算难度归一化分（那本身就是 R7 禁止的判断）。v1 **原样暴露每条邻居记录**（"这条相似过往任务 → 这个模型 → 这些原始指标 → 这个 terminate_reason"），由 LLM 对照它能看见的当前任务**在上下文内归一化**。spec 显式声明此点，防止实现者加 normalizer。残留弱信号风险写入 §11。

### D8. 不建也不预留反馈覆盖层
v1 **不实现** LLM-judge / user-signal 覆盖层，且**不建 trait、不留 seam、不放 placeholder**。零现有消费者的抽象即 R10 反模式。仅当出现第二个真实信号源时才新增抽象。

### D9. 保留与隐私：自有 cap/age-out，per-agent 作用域，绝不复用 memory GC
routing store **绝不**继承 memory 的失效/软删/decay 语义（耦合红线）；但因此它不会自然收缩，故**自带保留上界**（行数 cap 或按 `created_at` age-out 的简单轮转，非 memory daemon 驱动）。召回**按 `agent_id` 隔离**（复用 notes_vec 的 agent 维度隔离范式），使 LAN 暴露部署（gateway host `0.0.0.0`）下一个 principal 的任务语义不经注入邻居泄漏给另一个。具体上界数值与是否纳入 namespace 维度见 §11。

---

## 9. 成功标准与验收（具体、可测）

### 单元测试
- **U1 store kNN + 隔离**：插入 N 个合成 embedding（含 model_id/原始 outcome 列），`recall_routing_experience(q, dim, agent_id, k)` 返回**按 L2 距离升序**的最近邻；断言写入 `routing_exp_vec_768` 维度表、与 `notes_vec_*` 隔离；断言跨 `agent_id` 不串。
- **U2 observer 纯观测映射**：喂合成 `SessionCompleted`（`tool_timeline` 含 k 个 `success=false`）→ 断言 `tool_error_count == k`、`tool_call_total == len`、`terminate_reason` **原样**映射判别式 + 内嵌字段（如 `VerifierVeto{vetos}`）；**断言 store 记录里不存在任何 `success`/`score`/`rank` 列、`user_re_steer` 不被捏造、`consecutive_errors` 不被读取**。
- **U3 sink 构造期模型归因**：构造期注入 `(model_id, provider_id)` 的 OutcomeObserver，喂 `SessionCompleted` → 断言 `store.record` 收到的 model_id/provider_id 等于构造期注入值，且**不读 ProviderUsage**（回退方案启用时另测 `ProviderUsage.model_id == req.model`）。
- **U4 冷启动**：空 store → `recall` 返回 `None`/空 → `build_routing_experience_message` 返回 `None`（无注入），行为等同 baseline。
- **U5 record/recall 对称**：同一 user_query 文本在 record 与 recall 端产生同一 embedding key（断言派生路径一致）。

### 连线（wire-level）测试
- **W1 run-start 注入**：`build_routing_experience_message` 产出 `UnifiedMessage::user` 且被 fence 包裹，经 `builder.with_routing_experience_message` 注入**恰一次**；断言 `think.rs` 路径**从不**调用它（防每轮重注入）。
- **W2 活跃目录对账**：召回含已撤凭证模型 → 注入块将其标记 `unavailable`（或过滤），断言 LLM 永不收到可被 `select_model` 选中的死偏好。
- **W3 spawn 面**：父上下文含经验注入 + `list_models` 暴露 per-model 原始聚合（无 success_rate/best_for）；`SpawnRequest.model` 精度链生效（explicit → model_hint → native）。

### 端到端
- **E1 两相似任务经验累积**：跑任务 A 用模型 M（observer 记录 outcome）→ 跑语义相似任务 B → 断言 `RoutingRecall` 为 B 召回 M 的经验（kNN 命中），证明经验累积闭环。
- **E2 归因正确 + 父子隔离**：A run 用 M、B(子) run 用 N → 断言各自 outcome 归因到正确冻结模型；断言父记录不吸收子指标。

### 非回归
- **N1 harness 预算**：`src/harness/` 文件数仍为 **12**；首选方案下 `trace.rs` **diff 为空**；harness 内无新增逻辑分支。（若启用 §7 回退，单独 PR 论证两字段。）
- **N2 select_model 不变**：`select_model.rs` diff 为空（纯 passthrough 保持）。
- **N3 list_models 纯度**：enrich 新增字段全为中性原始聚合，无 success/best_for/ranking；既有纯 capability/rate 投影不变。

---

## 10. 熵减 / 旧代码审计

- **`consecutive_errors`（vestigial）**：`LoopTraceTurnMetrics`（`trace.rs:179-186`）恒 0，`think.rs:~850` 硬编码——**不消费**；标注为死字段但**不删除**（非本任务范围，pre-existing dead code，按 surgical 原则只提及）。
- **`hit_limit`（deprecated）**：`FlowOutcome`（`dispatch.rs:63-112`）字段已弃——VESR **不依赖**，改用 `terminate_reason::HitMaxIterations{used}`。
- **不重复 embedder**：审计确保全程复用唯一 `Arc<dyn EmbeddingProvider>`（`embedding_provider.rs:10-27`），**不实例化第二个 embedder**（违 R3）。
- **不污染 MemoryFact 生命周期**：审计确保 routing store 零调用 `invalidate`/`set_valid_from/to`/`access_count` 等 MemoryFact 方法（核验 RISK 第 5 点：开发者不应困惑 memory GC 是否影响路由）。
- **不在 src/routing 起子目录帝国**：复用既有 `src/routing/`（已含 `session_key`），平铺新增文件。
- **预存 stub 排查**：`grep src/routing`、检查是否有半成品路由经验存根或被取代的 select_model 历史分支。
- **run-start trace 事件核查**：审计 `LoopTraceEvent` 完整 taxonomy（`trace.rs:24-125`），确认是否存在携带 user_query 的 run-start 事件——若有，可考虑简化掉 `RoutingAttribution` 句柄（见 §11 O1）。

---

## 11. 风险与开放问题

| # | 议题 | 现状 / v1 倾向 |
|---|------|-----------|
| **O1** | **归因关联机制**：`SessionCompleted` 不带 user_query/session_id；当前方案=每-run `RoutingAttribution` 句柄（recall 写 task_emb、observer 读），模型经 §7 sink 构造期捕获。 | 倾向句柄方案（sink 每-run 构造已确认 `inner.rs:644-647`，guaranteed grounded）。**plan 须核查**是否存在携带 query 的 run-start 事件，若有可简化掉句柄。 |
| **O2** | **run 内多模型污染** | **已由 §7 sink 构造期捕获消解**——只归因冻结主模型，不读 ProviderUsage。保留此条仅作记录；若启用 §7 回退方案，需在 observer 侧过滤工具内部模型。 |
| **O3（MISSING：normalization）** | **跨任务难度归一化**：'8 iterations'/'$0.12' 无难度基线则误导（廉价模型只接简单活会"战绩好看"）。 | **v1 锁定 D7：不在代码内归一化**，原样暴露每条邻居记录由 LLM 上下文内对照当前任务归一化。残留：稀疏邻域仍可能弱信号→由 distance/sample/recency 让 LLM 折扣。代码侧难度分=违 R7，永不做。 |
| **O4（MISSING：unavailable）** | **召回模型已不可用**：经验里的模型可能已撤凭证/下线。 | **v1 锁定**：recall 与 `list_models` 的 `provider_configured` 门对账。**倾向标记 `unavailable`** 而非硬删（保留历史可见性）。"标记 vs 过滤"最终形态**开放**。 |
| **O5（MISSING：retention/privacy）** | **表无界增长 + LAN 跨主体泄漏**：store 不继承 memory GC（不该过期）却因此无界；`0.0.0.0` 下跨 agent 召回可能泄漏任务语义。 | **v1 锁定 D9**：自有 cap/age-out（非 memory daemon）+ per-agent 召回隔离。**具体上界数值、是否纳 namespace 维度开放。** |
| **O6（MISSING：multi-agent attribution）** | **父子归因**：delegation-heavy 父 run 的 cost/iterations 含子代理开销。 | **v1 锁定**：每 run 自归因到自身冻结模型 + 自身 task_emb，父不吸收子、子不写父；与 `RawMemory(Delegation)` 正交不重复。**残留**：父原始指标天然含等待子开销，v1 原样落库由 LLM 辨识，不归一化。 |
| **O7（MISSING：task-key drift）** | **embed 什么文本 + 多轮漂移**：跟进消息"ok now do X" embedding 近空。 | **v1 锁定 D6**：key=本 run user_query，record/recall 严格对称。**残留**：多轮漂移接受，不在 v1 引入"运行任务摘要"key（避免提前抽象），待数据验证。 |
| **O8（MISSING：exploration）** | **自强化死锁**：无 bandit → 邻域一旦偏好 X 就只产 X 数据，冷模型恒冷，"治信息缺失"自限。 | **v1**：recall 显式暴露 per-model **data-absence/sparsity**，让 LLM 可主动试未测模型（探索是 LLM 的决定，R7-clean）。**不引入 epsilon**。此 tradeoff 明确命名，不隐藏。 |
| **O9** | **稀疏 store 弱邻 + 小样本噪声**：小 store 的 kNN 返回远邻质量低；1-2 样本易过度自信。 | recall 每条附 `distance + n_runs + recency`，让 LLM 折扣。**是否硬抑制极远邻**（卫生过滤 vs 滑向 policy）——**开放，倾向只暴露不硬截**；**绝不** bake 最小样本门限（=policy 旋钮）。 |
| **O10** | **list_models enrich 由纯变异步**：原 `enrich()` 无 I/O，新增 store 查询引入 async + I/O。 | 接受：新增独立异步统计 pass，不破坏既有纯 capability/rate 投影；未知字段 `None`。需确认不显著拖慢工具响应。 |
| **O11** | **子代理召回投递通道**：list_models 全局聚合 vs delegation-memory 注入 vs 父上下文专用 recall 块（`wrap_memory_context` seam，核验 `subagent_spawn_model_seam_verification`）。 | v1：run-start 注入覆盖父自身模型，list_models 全局聚合供子代理模型选择。三选一最终形态**开放**。 |