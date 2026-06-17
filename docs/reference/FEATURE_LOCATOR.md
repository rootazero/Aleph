# FEATURE_LOCATOR.md — 功能定位词典

> **用途**：把"打磨某个功能时的口语关键词"翻译成**代码规范名 + 文件锚点 + 精准话术**，让 Claude 在局部优化时一次定位到正确的模块/文件，而不是从头摸索。
>
> **架构主轴**：Aleph agent 按 **Prompt → Context → Harness → Loop** 四层构建。本词典按这四层 + **横切关注点** + **UI/Panel** 两个补充区组织。
>
> **怎么用**：
> 1. 打磨前先在「速查索引」里用你的口语关键词找到**规范名**；
> 2. 跳到对应词条，把里面的**代码锚点**和**打磨话术模板**贴进你的需求描述；
> 3. 留意**实现状态**标记——`⚠️`/`❌` 的条目说明现状与直觉/旧描述有出入，先看「实现现状体检」一节。
>
> **状态图例**：✅ 已实现且与描述一致 ｜ ⚠️ 部分实现或与描述有出入 ｜ ❌ 未实现或描述不符
>
> **生成方式**：基于 2026-06-16 时点代码逐条查证（6 路并行 Explore）。锚点会随重构漂移；发现不符请就地更新本表。

---

## 0. 速查索引（口语关键词 → 规范名 → 状态）

| 层 | 你的口语关键词 | 规范名 (EN) | 主锚点 | 状态 |
|----|----------------|-------------|--------|------|
| Prompt | 系统提示词 / 提示词通用简洁精准 / 信息密度 | System Prompt Pipeline | `src/thinker/prompt_pipeline.rs` + `layers/` | ✅ |
| Prompt | prompt 预算 / token 控制 / prompt 大小 | Prompt Budget | `src/thinker/prompt_budget.rs` | ✅ |
| Prompt | 人格 / soul / 语气 | Identity & Soul | `src/thinker/soul.rs` `identity.rs` `layers/soul.rs` | ✅ |
| Context | 对话历史压缩 / token 控制 | History Compaction | `src/context/compact/compactor.rs` | ✅ |
| Context | kimi 20w vs claude 100w 压缩时机 + per-model 阈值 | Model-Aware Compaction Timing | `src/context/budget/pressure.rs` · `deps_builder.rs::build_context_budget_config` | ✅ |
| Context | context-mode / codex 上下文模式 | Context Mode | `src/thinker/context.rs` + `layers/execution_plan.rs` | ✅ |
| Context | voice 作为 context 层 | Voice-as-Context | `src/thinker/layers/voice_mode.rs` | ✅ |
| Context | 记忆三支柱①关键词链接 | Note Keyword Linking | `src/memory/notes/graph/relevance.rs` | ✅ |
| Context | 记忆三支柱②会话结束 flush | Session-End Flush | `src/memory/flush/mod.rs` | ✅ |
| Context | 记忆三支柱③错误沉淀教训 | Correction & Lesson Sedimentation | `src/memory/dreaming/stages/feedback_distill.rs` | ✅ |
| Harness | harness 架构 / Think-Act 笨循环 | Harness Architecture | `src/harness/` (12 文件) | ✅ |
| Harness | tool calling 2.0 / tool use | Tool Calling | `src/harness/agent/act.rs` + `src/tools/scoped/` | ✅ |
| Harness | 工具并发(群分) ≠ 任务 DAG | Tool Concurrency vs Task DAG | `src/tools/concurrency.rs`(工具群分) · `src/workflow/compile.rs`+`src/teams/dispatcher/`(任务 DAG) | ✅ 已澄清(G5) |
| Harness | built-in 工具 read/write file | Builtin File Tools | `src/builtin_tools/file_ops/` | ✅ |
| Harness | 内置命令注册机制 | Tool Registry | `src/tools/registry.rs` `runtime.rs` | ✅ |
| Harness | 统一工具注册 / 斜杠命令 | Unified Tool / Slash Command | `src/command/` + `src/tool_metadata/` | ✅ |
| Harness | AI 动态路由 | LLM-Driven Routing | `src/harness/agent/prompt.rs` (LLM 主权) | ✅ |
| Harness | shell / bash 工具 | Shell/Bash Tool | `src/builtin_tools/bash_exec.rs` | ✅ |
| Harness | sandbox shell 安全 / 命令策略 | Sandbox Command Policy | `src/sandbox/command_policy/` | ✅ |
| Harness | MCP | MCP Integration | `src/mcp/` | ✅ |
| Harness | plugins 插件 | Plugin System | `src/extension/` | ✅ |
| Harness | skills 技能 | Skill System | `src/skill/` | ✅ |
| Harness | browser 浏览器 | Browser Automation | `src/browser/` | ✅ |
| Loop | goal 命令 | Standing Goal | `src/goal/` + `src/builtin_tools/goal*` | ✅ |
| Loop | loop 命令 | Loop Command | `src/looping/` + `src/builtin_tools/loop_manage/` | ✅ |
| Loop | workflow 命令 | Workflow | `src/workflow/` | ✅ |
| Loop | task 任务管理 / 分解 / 验证 / 收尾 | Coordinated Tasks | `src/agents/swarm/tasks/` + `src/teams/dispatcher/` | ✅ |
| Loop | multi-agent / teams / 多代理 | Teams / Multi-Agent | `src/teams/` + `src/agents/` | ✅ |
| Loop | agent 切换 | Agent Switching | `src/builtin_tools/agent_manage/` | ✅ |
| Loop | 消息流 / 最终结果汇总打印 | Message Stream & Final Answer | `src/gateway/event_emitter/` | ✅ |
| Loop | 新消息排队 / 插入 / 改需求打断 | Message Queue & Steering | `src/gateway/session_scheduler.rs` `lane.rs` | ✅ |
| 横切 | 安全模块 | Security Primitives | `src/security/` + `src/pii/` | ✅ |
| 横切 | 全局/agent/channel 三级权限 | Permission Hierarchy | `src/approval/` | ✅ |
| 横切 | LLM 与用户互动 / 确认 / 授权 | LLM-User Interaction | `src/clarification/` + `src/event/permission.rs` | ✅ |
| 横切 | 预设 provider/model / 别名 / 成本路由 | Provider & Model Catalog | `src/providers/presets/` `model_catalog/` | ✅ |
| 横切 | gateway 集群 | Cluster | `src/cluster/` | ✅ |
| 横切 | channel 与 webchat 多端同步 | Channel Sync | `src/gateway/channel_registry.rs` | ✅ |
| 横切 | 打字机模式 / 即时输出全局开关 | Output Mode | `src/config/types/general.rs` + `event_emitter/instant_buffer.rs` | ✅ |
| 横切 | self 自我管理 | Self-Config / Self-Manage | `src/builtin_tools/self_config.rs` `self_manage.rs` | ✅ |
| 横切 | doctor / doctor+f | Doctor & Auto-Fix | `src/builtin_tools/doctor.rs` + `interfaces/webchat/src/state/hotkey.rs`(`f`) | ✅ (G1 已实现 2026-06-16) |
| 横切 | hook | Hook System | `src/verification/stop_hooks.rs` `src/sandbox/hooks.rs` | ✅ |
| 横切 | CLI | Command Line Interface | `src/bin/aleph-server/commands/` | ✅ |
| UI | 流式回显 / 工作区面板 | Streaming Echo & Workspace Panel | `interfaces/webchat/src/components/workspace_panel.rs` | ✅ |
| UI | panel 双层权限 / 配对 tier | Panel Permission Tiers | `src/gateway/panel_devices.rs`(tier store) · `src/gateway/server/handler.rs`(connect 解析+RPC门) · `src/gateway/method_authz.rs`(rpc/tool_requires_operator) · `src/gateway/handlers/devices.rs`(devices.*) · `interfaces/webchat/src/{context.rs(device_id),views/settings/security/devices.rs,components/permission.rs(ConfigGate)}` | ✅ |

---

## 1. Prompt 层

### 1.1 系统提示词构建管线 (System Prompt Pipeline)
- **口语关键词**：系统提示词、提示词组装、prompt assembly、通用性/简洁性/精准性、信息密度
- **代码锚点**：`src/thinker/prompt_pipeline.rs`（按优先级跑 layers）、`src/thinker/prompt_layer.rs`（Layer trait + AssemblyPath）、`src/thinker/layers/`（40+ 具体 layer，优先级 50→1755）、`src/thinker/prompt_builder/mod.rs`（输出与缓存）
- **职责**：按优先级排序的 layer 流水线，分 Basic/Soul/Context/Cached 路径组装静态+动态内容，产出完整系统提示。
- **状态**：✅ 已实现，含 layer 稳定性分化（Stable/Dynamic 驱动缓存分层）。
- **打磨话术**：「调系统提示词的某段内容 → 找对应 `src/thinker/layers/<X>.rs`（按 priority）。要改‘文字越少信息密度不变’这类全局风格 → 改 `soul.rs` 的 voice/directives 或对应 layer 的模板，不要散落式硬编码。」

### 1.2 Prompt 预算与 Token 控制 (Prompt Budget)
- **口语关键词**：prompt 预算、token 控制、prompt 大小、truncation、bootstrap 上限
- **代码锚点**：`src/thinker/prompt_budget.rs`（TokenBudget + TruncationStat，字符级截断 `max_total_chars`/`max_bootstrap_chars`）；与消息侧压力区分见 §2.1。
- **职责**：系统提示侧按字符上限截断，保证 prompt 不超预算。
- **状态**：✅ 已实现（CJK/code 内容感知）。
- **打磨话术**：「这是‘系统提示’侧的字符预算（`prompt_budget.rs`），不要和‘对话历史’侧的 token 压力（`context/budget/`）搞混——两层独立。」

### 1.3 人格与灵魂注入 (Identity & Soul)
- **口语关键词**：人格、soul、identity、语气、persona、声调
- **代码锚点**：`src/thinker/soul.rs`（SoulManifest：identity/voice/directives）、`src/thinker/identity.rs`（三层优先级：session override > 全局 > 默认）、`src/thinker/layers/soul.rs`（SoulLayer @priority50，SOUL.md 优先）、`src/thinker/identity_files.rs`
- **职责**：workspace `SOUL.md` / 全局 `~/.aleph/soul.md` / session override 三层堆栈，注入身份、语气、冗长度、格式化方针。
- **状态**：✅ 已实现。
- **打磨话术**：「改人格/语气走 SoulLayer 与 SOUL.md，三层优先级在 `identity.rs`。」

---

## 2. Context 层

### 2.1 对话历史压缩 (History Compaction)
- **口语关键词**：对话压缩、history compaction、session summary、窗口管理、token 有效控制
- **代码锚点**：`src/context/compact/compactor.rs`（三策略：LlmSummary / DeterministicTruncation / SessionMemoryReuse）、`src/context/compact/session_split.rs`（压缩失败后 split 新 epoch）、`src/context/budget/mod.rs`（ContextPressure 计算）、`src/context/budget/pressure.rs`（内容感知 token 估计）
- **职责**：消息历史压力超 warning 阈值时走侧信道 LLM 摘要，保留 fresh_tail（最近 ~6 条），失败回退确定性截断或旧 summary，极限时 split 新 session。
- **状态**：✅ 已实现，三层降级 + 缓存复用 + 零 API 成本路径。
- **打磨话术**：「‘记忆有效传递又控 token’的核心在 `compactor.rs` 的三策略降级；‘何时触发’在 `budget/pressure.rs` 的阈值。」

### 2.2 按模型窗口的压缩时机 (Model-Aware Compaction Timing)
- **口语关键词**：kimi 20 万 vs claude 100 万、不同模型不同压缩时机、模型窗口差异、压缩阈值、per-model 阈值
- **代码锚点**：`src/context/budget/mod.rs`（ContextPressure.compute 按 `token_budget` 参数化）、`src/context/budget/pressure.rs`（按 ratio 动态调整）；预算尺寸 + per-model 阈值在 `src/orchestrator/deps_builder.rs::build_context_budget_config`（`derive_chain_min_budget` 取链上最小窗口模型）；config 类型 `src/config/types/phase6_wiring.rs`（`ContextBudgetToml.model_thresholds` + `ModelThresholdToml` + `threshold_override_for`）。
- **职责**：按当前模型的 token_budget 计算压力比，warning/critical 阈值**相对该预算**触发，100 万窗口比 20 万窗口更晚触发；**且**可按模型覆盖 warning/critical 触发分数。
- **状态**：✅ **已实现（G4，2026-06-16）**——窗口尺寸自动浮动 **+** per-model 专属阈值映射。`[[context_budget.model_thresholds]]` 按"模型 id 或 provider key 的大小写不敏感子串"首匹配覆盖，未匹配/未配置字段逐项回退全局再回退内置 0.70/0.85（向后兼容）；解析后的阈值过同一 `0 < warning < critical ≤ 1.0` 防御闸（坏配置禁用而非降级）。阈值 key 在决定预算的链上最小窗口模型，二者自洽。
- **打磨话术**：「触发时机按 model token_budget 自动浮动（`pressure.rs`）；**要给某模型单独调阈值**用 `[[context_budget.model_thresholds]]`（matcher = model id / provider key 子串），连线在 `build_context_budget_config`。这是配置项不是代码改动。」

### 2.3 Context 模式 (Context Mode / codex 风格)
- **口语关键词**：context-mode、codex 上下文、环境感知、执行计划注入
- **代码锚点**：`src/thinker/context.rs`（ResolvedContext 四片段：execution_plan / standing_goal / voice_mode_active / sandbox_summary）、`src/thinker/layers/execution_plan.rs`（@priority1755）、`src/thinker/layers/standing_goal.rs`（@priority1754）
- **职责**：把 session 动态执行状态（活跃 plan / standing goal / voice 模式 / sandbox 摘要）作为结构化块注入提示词。
- **状态**：✅ 已实现。**注意**：代码里**没有名为 "context-mode" 的枚举**，而是 ResolvedContext 的四个字段按需组装。
- **打磨话术**：「‘context mode’落在 `ResolvedContext` 四片段 + 对应 layer，不是某个开关枚举。」

### 2.4 语音作为 Context 注入 (Voice-as-Context)
- **口语关键词**：voice 作为 context 层、语音模式提示词、TTS 口语风格
- **代码锚点**：注入侧 `src/thinker/layers/voice_mode.rs`（VoiceModeLayer @priority1710）+ `src/thinker/context.rs`（voice_mode_active）；写标志侧 `src/gateway/voice/session_mode.rs`。
- **职责**：voice 会话期写 `voice_mode_active=true`，VoiceModeLayer 注入"口语风格、避免 markdown、自然段落"的 TTS 指导。
- **状态**：✅ 已实现（注入侧）。
- **打磨话术**：「‘voice 作为 context 层’专指**提示词注入**（`layers/voice_mode.rs`）。语音运行时（ASR/TTS/格式化/流式）在 `src/gateway/voice/{inbound,outbound,format,streaming,local_provider}.rs`，两者分开，别混。」

### 2.5 记忆三支柱 (Memory Three Pillars)
> 三支柱是 Aleph 长期记忆的工程纪律，落在不同文件，分开描述更精准。

**① 关键词链接地基 (Note Keyword Linking)** ✅
- 锚点：`src/memory/notes/mod.rs`（frontmatter aliases/keywords）、`src/memory/notes/graph/relevance.rs`（四信号打分：相似度/引用频率/编辑模式/时间接近）、`src/memory/notes/graph/mod.rs`（community detection）
- 话术：「记忆链接地基 = 笔记 frontmatter 的 aliases/keywords + Note Graph 四信号相关性 + ingest 时自动 peer 链接。」

**② 会话结束实时 flush (Session-End Flush)** ✅
- 锚点：`src/memory/flush/mod.rs`（非阻塞 spawn `session_end_flush`）、`src/memory/flush/registry.rs`（FlushRegistry + await_ready）、`src/memory/compression/mod.rs`（compress_to_notes）
- 话术：「会话结束 flush = 非阻塞 spawn + FlushRegistry，让后续 session 可 await_ready，不阻塞当前 session end。」

**③ 纠正/教训即时沉淀 (Correction & Lesson Sedimentation)** ✅（G6 已查证 2026-06-16）
- **写入**：`src/builtin_tools/flag_user_correction.rs`（LLM 调的工具，写 `RawMemorySource::Correction` 到 `aleph://correction/{id}`）；构造于 `src/executor/builtin_registry/builder/constructor.rs:1793`（**有 `memory_db` 即注册，非死代码**），prompt 引导在 `src/thinker/layers/special_actions.rs`。
- **蒸馏**：`src/memory/dreaming/stages/feedback_distill.rs`（按 `aleph://correction/` 前缀 + watermark 幂等读 → LLM 蒸馏成 `feedback/` note），调度于 `src/memory/dreaming/mod.rs:172,218`（**Consolidate 每日 + Synthesize 两条 dream path 都挂**）。
- **召回**：`feedback/` note 由 assembler 表面化（`src/memory/assembler/gather.rs:284` / `envelope.rs:34`）；goal 教训另有 `GoalLessonsPromoteStage` → `lesson/` note。
- **状态**：✅ 端到端已连且生产存活（写入工具注册 + distill 双路调度 + 召回消费者，逐跳有单测）。
- **设计边界（重要）**：沉淀是 **LLM/工具驱动**（R8 工具即一切 / R7 LLM 主权）——LLM 判断"这值得记"才调 `flag_user_correction`。**没有也不应有**"每次工具失败自动写 raw memory"的 harness 错误 hook（违 R10「不做错误恢复」+ R7，且会用瞬时报错噪声淹没记忆）。
- 话术：「‘错误/纠正沉淀’走 `flag_user_correction` + `FeedbackDistill`，已全连且存活。想要‘自动捕获工具失败 → 教训’——**这是故意不做的设计边界**（R7/R10），别加 harness 错误 hook；要让 LLM 多记教训就强化 prompt 引导它调工具。」

---

## 3. Harness 层

### 3.1 Harness 架构 (Harness Architecture)
- **口语关键词**：harness 架构、Think→Act、笨循环、薄 harness、调度骨架
- **代码锚点**：`src/harness/`——`mod.rs`（8 导出）、`agent.rs`、`agent/think.rs`（LLM 调用+守卫+验证）、`agent/act.rs`（工具执行+并行）、`agent/guardrails.rs`、`agent/prompt.rs`（逐轮消息组装）、`deps.rs`、`trait_def.rs`（Harness trait + TurnState）、`callback.rs`、`chain_context.rs`、`trace.rs`、`trace_sink.rs`
- **职责**：驱动 Think→Act 轮次，管预算/守卫/验证；零意图分类、零工具过滤、零完成度判断（交 LLM + prompt，R7/R9/R10）。
- **状态**：✅ 已实现，受 **CLAUDE.md R10 红线**约束（限 12 文件 / ~4900 行预算）。改这里前先回答"加代码前必答 3 问"。
- **打磨话术**：「harness 只管调度，**不要往里加推理逻辑**（违 R10）。要改‘循环行为’找 `agent/think.rs`/`act.rs`；要改‘轮次状态机’找 `trait_def.rs` 的 TurnState。」

### 3.2 Tool Calling 2.0 / Tool Use
- **口语关键词**：工具调用、native tool call、并行、结果缓存、result store
- **代码锚点**：`src/harness/agent/act.rs`（执行管道：缓存、并行分组、失败处理）、`src/tools/scoped/`（ScopedToolService：权限/确认/hook/result store）、`src/tools/scoped/dispatch.rs`（pre/post hook + 溢出持久化）、`src/providers/adapter.rs`（NativeToolCall）、`src/tools/runtime.rs`（LoopTool trait）
- **职责**：LLM 发原生 tool_call → harness 分批并/序执行 → 缓存重复调用 → 超大结果持久化 → 回 ToolResult/ToolError 事件。
- **状态**：✅ 已实现，三层管道（act 分组并行 / ScopedToolService 拦截 / ToolResultStore 溢出）。
- **打磨话术**：「工具执行三层：`act.rs`(并行保序) → `scoped/`(权限确认 hook) → result store(溢出)。改‘工具结果太大被截断’找 result store；改‘确认弹窗’找 scoped。」

### 3.3 工具并发调度 (Tool Concurrency)
- **口语关键词**：DAG 工具执行、并行分组、智能调度、资源作用域、并发安全
- **代码锚点**：`src/tools/concurrency.rs`（partition_parallel_groups + ConcurrencyClaim：Shared / Exclusive{Paths/Global}）、`src/tools/runtime.rs`（LoopTool::concurrency_claim）、`src/builtin_tools/file_ops/tool.rs`（按操作类型声明 claim）
- **职责**：工具按资源作用域声明，harness 群分保证无冲突资源并行。
- **状态**：✅ **已澄清（G5，2026-06-16）**——工具层是"群分顺序"而非完整 DAG：按资源作用域分群（`Shared` / `Exclusive{Global, Paths}`）、群内并行群间串行，**没有完整依赖图解析**。`concurrency.rs` 头部自述是"a data-race guard, not an LLM judgement … this only schedules them"（守 R7/R10：不做意图推理/相关性评分/工具过滤）。"智能调度"= 资源冲突避免，**不是**任务 DAG。**真正的任务级 DAG 在**：§4.3 Workflow（`src/workflow/compile.rs`：`step.depends_on → coord_task.blocked_by`，拓扑序物化）/ §4.4 Task（`src/teams/dispatcher/` 按 `blocked_by` 边扫描 Runnable 并发调度）。
- **打磨话术**：「‘DAG 工具执行’在工具层其实是 `concurrency.rs` 的资源群分并行（非真 DAG）。要‘多步骤依赖图’去 Workflow/Task 层（`compile.rs`+`teams/dispatcher/`），**别在 `concurrency.rs` 找、也别在工具层重造 DAG**（违 R6；该需求应上升到 Workflow 层表达）。」

### 3.4 内置文件工具 (Builtin File Tools)
- **口语关键词**：built-in 工具、read/write file、edit、apply patch、文件操作
- **代码锚点**：`src/builtin_tools/file_ops/`——`tool.rs`（FileOps 8 operation dispatch）、`read.rs`、`write.rs`、`edit.rs`、`apply_patch.rs`、`mod.rs`（check_path 保护黑名单 .ssh/.aws 等）；`src/builtin_tools/bash_exec.rs`、`src/builtin_tools/generation/`（图像/语音）
- **职责**：文件读写/编辑/补丁 + bash + 生成类原子工具，impl AlephTool 自动生成 schema。
- **状态**：✅ 已实现，路径黑名单 + 沙箱 policy 双守卫。
- **打磨话术**：「内置 file 工具都在 `file_ops/`，每个操作一个文件。路径安全在 `mod.rs::check_path`。」

### 3.5 工具注册机制 (Tool Registry) & 统一工具/斜杠命令
- **口语关键词**：内置命令注册、工具注册、统一注册、斜杠命令、slash command、热加载
- **代码锚点**：`src/tools/registry.rs`（ToolHandlerRegistry，ArcSwap 无锁热加载 + subscribe 变更广播）、`src/tools/runtime.rs`（LoopToolRegistry，gateway 主表）、`src/command/`（parser.rs 解析 `/input`，types.rs CommandNode 扁平命名空间 + source_type）、`src/tool_metadata/`（ToolCatalog 聚合 builtin/MCP/skill/prompt）
- **职责**：双层注册表（harness 内层 ToolHandlerRegistry + gateway 层 LoopToolRegistry）；斜杠命令解析**委托同一个 ToolCatalog**，命令与工具**同源不双维护**（R8 工具即一切）。
- **状态**：✅ 已实现。**关键事实**：'内置命令注册'、'统一工具注册'、'斜杠命令'在代码里是**同一套**（ToolCatalog 单源），不是三套独立系统。
- **打磨话术**：「斜杠命令 = 工具（同源 ToolCatalog）。加新斜杠命令 = 注册新工具，不要另起命令树。热加载在 `registry.rs` 的 ArcSwap。」

### 3.6 AI 动态路由 (LLM-Driven Routing)
- **口语关键词**：AI 动态路由、意图路由、工具选择、语义路由
- **代码锚点**：`src/harness/agent/prompt.rs`（把工具 schema 列表注入 system prompt）、`src/harness/agent/think.rs`（`.with_tools()` 发给 LLM）、`src/builtin_tools/gateway_route.rs`（**纯确定性 channel→agent 解析查询**，不分类意图）、`src/routing/resolve.rs`（`resolve_route` 层级匹配引擎）
- **职责**：把全部可用工具 schema 注入提示词，**由 LLM 自由选择/组合**；系统不做确定性意图分类或工具过滤。`gateway_route` 只回答"这条消息按 channel/peer 绑定路由到哪个 agent/session"，是配置驱动的 I/O 查询，不碰语义。
- **状态**：✅ 已实现（LLM 主权 R7）。
- **打磨话术**：「‘动态路由’= LLM 看全量工具自选（`prompt.rs` 注入）。**不要加规则引擎式意图分类**（违 R7）。`gateway_route` 是确定性 channel 解析，不是意图分类器。**已熵减（2026-06-17）**：删除寄生的 regex 任务分类器（旧 `routing/rules.rs` + `routing/task_router.rs` + `tool_metadata` 的 L1/L2/L3 `RoutingLayer` + 死配置 `[task_routing]`）——它们是 Dispatcher 解散遗骸、suggestion-only 无消费者、直接违 R7/P8，已连根清除。」

### 3.7 Shell/Bash 工具 (Shell Execution)
- **口语关键词**：bash、shell、脚本执行、后台进程、wait/poll/kill、后台进程上限
- **代码锚点**：`src/builtin_tools/bash_exec.rs`（BashExecTool + spawn_background + handle_process_action）、`src/builtin_tools/process_registry.rs`（后台进程表：register/poll/**wait**/kill/list + 每会话运行上限 + 完成 Notify）、`src/builtin_tools/code_exec.rs`（通用执行器）、`src/sandbox/workspace.rs`（执行环境）
- **职责**：沙箱隔离的 shell 执行，支持多行脚本、后台进程（poll/**wait**/kill/list）、超时；后台进程**每会话至多 8 个运行中**（`MAX_RUNNING_PER_SESSION`），超限拒绝并引导 poll/kill。
- **状态**：✅ 已实现。**后台增强（2026-06-17）**：① `process_action:"wait"` 用 Tokio `Notify` 阻塞等待完成（非忙轮询，默认 60s 上限 170s，回到前台 180s 预算内）；② 每会话运行中进程上限（修复 `evict_if_needed` 只淘汰已完成条目 → 运行态可无界增长的资源泄漏）。
- **打磨话术**：「bash 工具本体在 `bash_exec.rs`；后台进程生命周期/上限/wait 在 `process_registry.rs`；‘命令安不安全’是另一回事，见 §3.8 沙箱策略。要调后台并发上限改 `MAX_RUNNING_PER_SESSION`；要调 wait 窗口改 `WAIT_DEFAULT/MAX_TIMEOUT_SECS`。」

### 3.8 沙箱命令策略 (Sandbox Command Policy)
- **口语关键词**：sandbox shell 安全、命令过滤、危险命令、hardline、反混淆、policy
- **代码锚点**：`src/sandbox/command_policy/`（mod.rs 引擎、rules.rs 规则集、normalize.rs 反混淆）、`src/sandbox/scrub.rs`（输出秘密清理）、`src/sandbox/hooks.rs`（SandboxBeforeHook 集成）、`src/sandbox/policy.rs`、`exec_approval/`、`deny_globs.rs`
- **职责**：OS 沙箱之前的**内容层**防御：正则硬过滤，分 hardline（不可绕过：fork-bomb/dd/mkfs/rm --no-preserve-root/wipefs·blkdiscard·shred 设备擦除/Windows 灾难形）与 tunable（block/warn/off 三态）；命令先 normalize 反混淆（零宽符/反斜杠/脱字符/反引号/空引号）。
- **状态**：✅ 已实现（2026-06-17 强化：① 修复设备类绕过——`dd`/`>` redirect 漏 `/dev/xvd*`(AWS EC2 根盘)·`dm-`·`md`·`pmem`·`sr`·`loop`，统一并补齐；② 新增 hardline `device_wipe_tools`(wipefs/blkdiscard/shred→/dev/)；③ 新增 tunable warn `shell_eval_download`(`bash <(curl…)` 进程替换 + `eval "$(curl…)"` 绕 `pipe_to_shell`)）。
- **打磨话术**：「改‘命令拦截规则’找 `command_policy/rules.rs`；‘灾难性底线’在 hardline_rules（即便关 enforcement 也生效）；‘绕过手法’防御在 `normalize.rs`。」

### 3.9 MCP 集成 (MCP Integration)
- **口语关键词**：MCP、外部 server、tools/resources/prompts、OAuth、sampling
- **代码锚点**：`src/mcp/`——`client.rs`（连接）、`manager/`（生命周期）、`transport`（Stdio/Http/Sse）、`tool_bridge`（动态注册 MCP 工具）、`resources`、`prompts`、`approval.rs`、`context_injector.rs`、`auth/`、`external/`、`preflight.rs`
- **职责**：标准 MCP 协议联接外部 server，发现并代理 tools/resources/prompts，支持 OAuth/采样/工具过滤/上下文注入/风险批准。
- **状态**：✅ 已实现。
- **打磨话术**：「MCP 全在 `src/mcp/`；‘MCP 工具如何进 Aleph 工具表’找 tool_bridge；‘外部 server 配置’找 `external/`。」

### 3.10 插件系统 (Plugin System)
- **口语关键词**：plugins、插件、WASM 插件、MCP 插件、marketplace、plugin.json
- **代码锚点**：`src/extension/`——`loader.rs`、`plugin_ops.rs`、`discovery/`、`manifest/`、`hooks/`、`marketplace/`、`capability.rs`、`types/plugins.rs`
- **职责**：管理 Wasm/Mcp/Static 三类插件的发现/加载/注册，多源优先级（Config > Workspace > Global > Bundled）、热重载、风险扫描、marketplace 安装。
- **状态**：✅ 已实现。**关键事实**：'plugins' 在代码里属于 **`src/extension/`**（plugin 是 extension 的一种 kind），不是独立 `src/plugins/`。
- **打磨话术**：「插件 = extension（`src/extension/`）。三类 kind：Wasm/Mcp/Static。改‘插件优先级/发现’找 discovery，与 Skill 共享优先级解析。」

### 3.11 技能系统 (Skill System)
- **口语关键词**：skills、技能、SKILL.md、资格评估、prompt 注入、共现
- **代码锚点**：`src/skill/`——`manifest.rs`（SKILL.md 解析）、`registry.rs`、`installer.rs`、`eligibility.rs`、`preprocess.rs`、`prompt.rs`（build_skills_prompt_xml）、`guard.rs`（安全扫描）、`cooccurrence.rs`
- **职责**：解析 SKILL.md → 评估资格 → 执行安装指令 → 注入 prompt → 跟踪使用与共现。
- **状态**：✅ 已实现，与插件共享源优先级（workspace > plugin > global > bundled）。
- **打磨话术**：「技能定义解析在 `manifest.rs`，‘何时把技能塞进 prompt’在 `eligibility.rs` + `prompt.rs`。」

### 3.12 浏览器自动化 (Browser Automation)
- **口语关键词**：browser、浏览器、screenshot、Chrome MCP、Playwright、网络策略
- **代码锚点**：`src/browser/`——`backend.rs`（BrowserBackend trait）、`chrome_mcp_backend.rs`、`playwright_cli_backend.rs`、`manager.rs`、`network_policy.rs`、`tab_registry.rs`、`secret_guard.rs`、`types.rs`
- **职责**：统一文本优先浏览器接口，双后端（Chrome DevTools MCP / Playwright CLI），截图/点击/导航/填表/JS/网络隔离/凭证过滤。
- **状态**：✅ 已实现。
- **打磨话术**：「浏览器双后端在 `backend.rs` trait 下；‘换后端/加操作’改对应 *_backend.rs；‘网络隔离’在 `network_policy.rs`。」

---

## 4. Loop 层

### 4.1 Goal 命令 (Standing Goal)
- **口语关键词**：goal 命令、自主目标、持久目标、自动续跑、迭代/token/deadline 上限
- **代码锚点**：`src/goal/`（mod.rs / types.rs / store.rs）、`src/tasks/goal_pursuit.rs`、`src/builtin_tools/goal*`（set/update/list/clear）
- **职责**：用户设持久目标，LLM 经 goal 工具管状态，后台按 迭代/token/deadline 上限自主续跑，每轮注入进度 lessons + 剩余配额。
- **状态**：✅ 已实现（should_continue / continuation_prompt / cap/deadline/budget_reached_note 全连，门控器决定客观完成）。
- **打磨话术**：「goal 状态机在 `src/goal/`；‘续跑触发’在 `tasks/goal_pursuit.rs`；用户面工具在 `builtin_tools/goal*`。」

### 4.2 Loop 命令 (Loop Command)
- **口语关键词**：loop 命令、周期循环、定时、cadence、内存态
- **代码锚点**：`src/looping/`（mod.rs / types.rs / pursuit.rs）、`src/builtin_tools/loop_manage/`（set/update/list/stop/clear）
- **职责**：内存 HashMap 维护每会话 LoopState（Fixed/Timeout），hook 按 next_wake 定时触发续跑 RPC。
- **状态**：✅ 已实现（含 fail-closed `stop_loop_on_failure` + update 原地重定速）。**注意**：状态**只存进程内，daemon 重启清零**（设计意图"随会话消亡"）。
- **打磨话术**：「loop 状态在 `src/looping/`，**内存态、重启丢失**别当持久。要持久周期任务用 cron（`src/tasks/cron/`）。」

### 4.3 Workflow 命令 (Workflow)
- **口语关键词**：workflow 命令、DAG 工作流、步骤模板、workflow.js 互转、per-step 模型覆盖、提案评审
- **代码锚点**：`src/workflow/`——`def.rs`（WorkflowDef）、`compile.rs`（materialize → coord_tasks + blocked_by 边；workflow_model_override）、`clarify.rs`（闸门）、`proposal.rs`（import/review/accept）、`store.rs`、`interop/`（.workflow.js）
- **职责**：声明式 WorkflowDef → 编译为 DAG coord_tasks → TeamDispatcher 按拓扑并发；每步可覆盖模型；支持 .workflow.js 无损互转 + 提案审批。
- **状态**：✅ 已实现（per-step model override 经 manifest → RunRequest.model_override，零 harness 侵入 R10）。
- **打磨话术**：「真正的多步骤 DAG 在 `src/workflow/`（不是工具层 concurrency）；‘编译成任务图’在 `compile.rs::materialize`。」

### 4.4 协调任务 (Coordinated Tasks)
- **口语关键词**：task 任务管理、规划、分解、子任务分配、实施、验证、收尾、僵尸任务
- **代码锚点**：`src/agents/swarm/tasks/`（coord_task.rs / store.rs / types.rs）、`src/teams/dispatcher/schedule.rs`（select_schedulable）、`src/teams/dispatcher/runner.rs`（execute_member_task）
- **职责**：DAG 中每个 CoordTask 按 blocked_by 扫描依赖，上游完成→Runnable，分派器选最闲 owner 并发执行，失败重试 3 次→FailedFinal，超时→僵尸强制失败。
- **状态**：✅ 已实现（CoordTaskState 四态 + DispatcherConfig：max_concurrent=4 / zombie_ttl=7200s / lock_ttl=900s）。**注意**：tasks 无直接用户工具，经 workflow/teams leader 间接驱动。
- **打磨话术**：「任务调度/依赖/重试/僵尸检测在 `teams/dispatcher/`；任务数据结构在 `agents/swarm/tasks/`。」

### 4.5 多代理 / 团队 (Teams / Multi-Agent)
- **口语关键词**：multi-agent、teams、多线程多任务多代理、leader、群聊广播、roster
- **代码锚点**：`src/teams/`——`dispatcher/`、`messages/`（路由）、`broadcast/mod.rs`（GroupChatBroadcaster::dispatch，MAX_CHAIN_DEPTH=6 / MAX_FANOUT_WIDTH=5）、`store.rs`、`leader_prompt.rs`、`workflow_canvas.rs`；`src/agents/`（registry/runtime/subagent_spawner/swarm）
- **职责**：leader 创建团队并分解任务（建 coord_tasks），成员并发执行，消息经 Aggregator 合并后 MessageRouter 投递，群聊可自主链式接话（深度+宽度限制）。
- **状态**：✅ 已实现。
- **打磨话术**：「‘多代理协作/群聊’在 `src/teams/`；‘单个 agent 怎么跑/怎么 spawn 子代理’在 `src/agents/`。两者配合。」

### 4.6 Agent 切换 (Agent Switching)
- **口语关键词**：agent 切换、创建/删除/列出、项目覆盖、agent 配置
- **代码锚点**：`src/builtin_tools/agent_manage/`（create/delete/list/info）、`src/agents/registry.rs`、`src/agents/loader.rs`（lookup_with_overlay 项目覆盖）
- **职责**：agent 工具管生命周期，全局（~/.aleph/agents/）+ 项目层（project/.aleph/agents/）两级，项目层可影子覆盖全局。
- **状态**：✅ 已实现。
- **打磨话术**：「agent 创建/切换 UI 工具在 `agent_manage/`；‘加载与项目覆盖’在 `agents/loader.rs`。」

### 4.7 消息流与最终答案汇总 (Message Stream & Final Answer)
- **口语关键词**：对话消息流、StreamEvent、最终结果汇总、final_response、RunComplete、汇总打印输出
- **代码锚点**：`src/gateway/event_emitter/`（types.rs StreamEvent、impls.rs、instant_buffer.rs）；最终答案提取 `src/teams/broadcast/mod.rs::extract_final_response()`、`src/tasks/cron/executor.rs::extract_final_response()`
- **职责**：执行中逐步发 StreamEvent（Reasoning/ToolStart/ResponseChunk/RunComplete），`RunComplete.RunSummary.final_response` 是最终文本，broadcast/cron 从事件日志提取后投递/打印。
- **状态**：✅ 已实现。**注意**：**最终答案没有独立的表**，靠扫描 StreamEvent 日志找 RunComplete 抽取。
- **打磨话术**：「‘最后那段汇总输出怎么来的’= harness 发 RunComplete 事件，消费方调 `extract_final_response()` 扫事件日志。没有‘答案表’，改投递逻辑去 broadcast/cron executor。」

### 4.8 消息排队与改需求打断 (Message Queue & Steering)
- **口语关键词**：新消息排队、插入策略、agent 执行中改需求、打断、插队、steering、lane 优先级
- **代码锚点**：`src/gateway/session_scheduler.rs`（per-session FIFO + active_run_id + MAX_QUEUE_AGE=5min）、`src/gateway/inbound_router/busy_queue.rs`（per-agent FIFO + 原子 ticket，上限 32/agent）、`src/gateway/lane.rs`（Lane 分类 Query/Execute/Mutate/System + ChannelClass 优先级）、`src/gateway/resume_coordinator.rs`、`src/gateway/cancellation.rs`
- **职责**：新消息入 SessionScheduler FIFO，空闲立即执行否则排队；agent 繁忙时入 busy_queue（限 32），仅队首尝试投递；Lane + ChannelClass 让 Panel 优先级高于 Bot；超龄任务丢弃返 429。用户执行中可调 steering 工具改目标或中止。
- **状态**：✅ 已实现。
- **打磨话术**：「‘用户改需求/插队/打断’的核心在 `session_scheduler.rs`（会话级 FIFO）+ `busy_queue.rs`（agent 级 FIFO）+ `lane.rs`（优先级）。要改‘改需求时是排队还是打断当前 run’就动这三处 + `cancellation.rs`。」

---

## 5. 横切关注点

### 5.1 安全原语 (Security Primitives)
- **口语关键词**：安全模块、注入防护、PII、内容净化、Unicode 守卫、SSRF、审计
- **代码锚点**：`src/security/`（mod.rs / injection_patterns.rs / content_sanitizer.rs / runtime_guard.rs / unicode_guard.rs / dangerous_tools.rs / ssrf/ / audit.rs）、`src/pii/`
- **职责**：横切安全库——HTTP 头、SSRF、内容净化、高保真 PII 过滤、审计日志，集中在 gateway 层。
- **状态**：✅ 已实现。
- **打磨话术**：「安全原语都在 `src/security/`，按子文件分（注入/PII/SSRF/unicode）。这是‘内容层’安全；‘命令层’安全在 §3.8 sandbox。」

### 5.2 三级权限 (Permission Hierarchy)
- **口语关键词**：全局/agent/channel 权限、工具权限、临时授予、escalation
- **代码锚点**：`src/approval/`（policy.rs / types.rs / session_route.rs / operator_requester.rs / node_requester.rs）、`src/event/permission.rs`、`src/config/types/policies/tool_permissions.rs`、`src/gateway/inbound_router/permission.rs`、`src/gateway/channel_policy.rs`
- **职责**：分级授权——工具级（action_type → block/allow/ask）、通道级（per-channel policy）、代理级（全局默认），支持临时授权记忆与跨节点 escalation。
- **状态**：✅ 已实现（ApprovalPolicy::check → Allow/Deny/Ask）。
- **打磨话术**：「三级权限引擎在 `src/approval/`；‘通道级覆盖’在 `gateway/channel_policy.rs`；‘人工确认/集群上报’在 operator_requester/node_requester。」

### 5.3 LLM 与用户互动 (LLM-User Interaction)
- **口语关键词**：确认消息、授权、clarification、ask_user、Halo 浮窗、permission request
- **代码锚点**：`src/clarification/`（mod.rs / session.rs）、`src/event/permission.rs`、`src/builtin_tools/ask_user.rs`
- **职责**：双路——Clarification（菜单/文本，Halo overlay）与 Permission（action 确认），均带 timeout + 默认值；用户选 Always 存规则。
- **状态**：✅ 已实现。
- **打磨话术**：「‘问用户选项/要信息’走 `ask_user` + `src/clarification/`；‘要授权确认’走 `event/permission.rs` + §5.2 approval。两路都在 Halo 浮窗渲染。」

### 5.4 预设 Provider 与模型路由 (Provider & Model Catalog)
- **口语关键词**：预设 provider、模型别名、规范化、能力门控、成本路由、failover、metadata
- **代码锚点**：`src/providers/presets/registry.rs`（PROFILES 单一源 + 别名展开）、`src/providers/model_catalog/`（alias.rs / endpoint.rs / 能力矩阵）、`src/providers/capability_gate.rs`、`src/providers/failover.rs`、`src/providers/metadata.rs`、`src/pricing.rs`
- **职责**：PROFILES 驱动预设别名（Kimi=Moonshot）；model_catalog 存能力矩阵；capability_gate 做 per-model 需求匹配；failover + pricing 驱动降级/选型。
- **状态**：✅ 已实现。
- **打磨话术**：「加/改预设别名在 `presets/registry.rs`；‘某模型支不支持 vision/tool-use’在 `model_catalog/` + `capability_gate.rs`；‘成本’在 `pricing.rs`。」

### 5.5 集群 (Cluster)
- **口语关键词**：gateway 集群、单中心非对称节点、反向 RPC、node_invoke、center approval
- **代码锚点**：`src/cluster/`（mod.rs / reverse_rpc.rs / registry.rs / node_approval.rs / node_runtime.rs）；文档 `docs/reference/CLUSTER.md`
- **职责**：单中心 + 边缘节点，节点主动连中心反向 RPC，agent 经 node_invoke 跨边界执行，高风险操作回中心人工 escalation，断线 fail-fast。
- **状态**：✅ Phase 0a（ReverseRpcChannel / NodeRegistry / CenterApprovalRequester 已实现；node_invoke 路由/环境聚合属 Phase 1+ 计划）。
- **打磨话术**：「集群在 `src/cluster/`；‘节点怎么连中心’看 reverse_rpc.rs；信任边界=LAN，中心↔节点无认证层。」

### 5.6 多端通道同步 (Channel Sync)
- **口语关键词**：channel 多端同步、webchat 同步、通道注册表、统一消息总线、delivery queue
- **代码锚点**：`src/gateway/channel_registry.rs`、`src/gateway/channel.rs`、`src/gateway/delivery_queue.rs`、`src/gateway/interfaces/`（telegram/discord/wechat/matrix/signal…）
- **职责**：中心 ChannelRegistry 管所有通道，inbound 统一广播进 event bus（所有 agent 可见），outbound 经 delivery queue（rate-limit retry ≤2 轮 cap 30s + 可选持久化）。
- **状态**：✅ 已实现。
- **打磨话术**：「‘消息怎么在多端同步’= inbound 广播进 event bus + outbound delivery queue。加新通道在 `gateway/interfaces/`；‘掉线重试/持久化’在 delivery_queue.rs。」

### 5.7 输出模式：打字机 / 即时 (Output Mode)
- **口语关键词**：打字机模式、流式输出、即时输出、全局开关、所有 channel 同步、output_mode
- **代码锚点**：`src/config/types/general.rs`（BehaviorConfig.output_mode + typing_speed）、`src/gateway/event_emitter/instant_buffer.rs`（instant 装饰器）、`src/gateway/handlers/agent.rs`（resolved_output_mode，每次 run fresh 读）、`src/gateway/session_scheduler.rs`、前端 `interfaces/webchat/src/components/markdown.rs`（StreamingRenderer）
- **职责**：全局 `behavior.output_mode` = typewriter（逐字符，可设速度）/ instant（整体返回）；instant 是 EventEmitter 装饰器包裹任意 inner emitter；Panel run 与 inbound channel run **同源**，改配置下次运行即生效无需重启。
- **状态**：✅ 已实现（这是真·全局开关 + 全通道同源，**与你的描述一致**）。
- **打磨话术**：「全局开关在 `config/types/general.rs` 的 output_mode；‘所有 channel 同步’靠 `handlers/agent.rs::resolved_output_mode` 每次 run fresh 读同一配置。前端呈现在 `webchat/.../markdown.rs`。」

### 5.8 自我管理 (Self-Config / Self-Manage)
- **口语关键词**：self 自我管理、自动配置、LLM 驱动配置、配置向导
- **代码锚点**：`src/builtin_tools/self_config.rs`（交互配置向导）、`src/builtin_tools/self_manage.rs`（LLM intent → 读 self SKILL.md 自管理手册）、`src/builtin_tools/doctor.rs`
- **职责**：self_manage 读 `~/.aleph/skills/self/SKILL.md` 导航自管理；self_config 交互式配置向导；密钥走 vault_store，结构改动触发 hot-reload。
- **状态**：✅ 已实现。
- **打磨话术**：「‘自我管理/自动配置’= self_manage + self_config 两工具；密钥不进 config.toml 走 vault。」

### 5.9 Doctor 诊断与修复 (Doctor & Auto-Fix)
- **口语关键词**：doctor、诊断、自动修复、doctor+f、机械修复
- **代码锚点**：`src/builtin_tools/doctor.rs`（DoctorArgs{ fix: bool }）、`src/bin/aleph-server/commands/doctor.rs`（`aleph doctor --fix`）、`src/diagnostics/`（engine.rs / finding.rs `Finding{repairable, fix_hint, repair_outcome}`）；**Panel `f` 入口**：`interfaces/webchat/src/state/hotkey.rs`（裸 `f` + `focus_is_editable()` 护栏 → `chat.request_repair()`）、`views/chat/state.rs`（`repair_pulse`/`request_repair`）、`views/chat/composer/mod.rs`（`DOCTOR_REPAIR_PROMPT` + 监听 Effect）。
- **职责**：fix=false 只读检查（路径/配置/锁/vault/shell-hook 同意/浏览器前置/provider 连通），fix=true 机械修复（建缺目录、清 stale lock）。不可机械修复项带 `fix_hint` 引导 LLM 走 self_config/vault_store。
- **状态**：✅ **G1 已实现 2026-06-16**。doctor 工具/diagnostics 后端**早已结构化**（`repairable`+`fix_hint`+`repair_outcome` 直接喂 LLM）；新增 **Panel `f` 入口**：焦点不在输入框时按 `f` → 注入一句诊断-修复 prompt（R9）走现有 send 管线 → agent loop 的 LLM 读 findings 并按 repairable/fix_hint 路由修复。**未在 doctor 内写修复分支**（守 R7/R10）。
- **打磨话术**：「`f` 入口已通（`hotkey.rs` 裸 `f`，带编辑焦点护栏）。'让 LLM 修复'= 注入 `DOCTOR_REPAIR_PROMPT` 走现有 loop+工具，不是 doctor 内的确定性修复——要调修复行为改 `composer/mod.rs` 的 prompt 常量（R9），别动 doctor 工具。」

### 5.10 Hook 系统 (Hook System)
- **口语关键词**：hook、stop hook、sandbox hook、extension hook、shell-hook consent、veto
- **代码锚点**：`src/verification/stop_hooks.rs`（StopHookHandler，可 veto 完成）、`src/sandbox/hooks.rs`（SandboxBefore/AfterHook）、`src/extension/hooks/consent.rs`（ShellHookConsent，~/.aleph/shell-hooks-allowlist.json SHA256 指纹）、`src/config/types/stop_hooks.rs`
- **职责**：三级——stop hooks（agent 停止前校验，可否决完成）、sandbox hooks（工具执行前后查能力）、shell-hook consent（plugin 注册的 shell 命令执行前需人工批准）。
- **状态**：✅ 已实现。
- **打磨话术**：「‘hook’在 Aleph 是三套：完成校验（`verification/stop_hooks.rs`）、工具前后（`sandbox/hooks.rs`）、plugin shell 同意（`extension/hooks/consent.rs`）。说‘hook’时指明哪一套。」

### 5.11 CLI (Command Line Interface)
- **口语关键词**：CLI、命令行、start/stop/status、子命令
- **代码锚点**：`src/bin/aleph-server/cli.rs`（Clap 定义）、`src/bin/aleph-server/commands/`（mod.rs 分发 + doctor/plugins/audit/hooks/secret/node/sandbox_debug/prompt_size/start）、`src/cli/ipc_client.rs`
- **职责**：Clap 驱动入口，覆盖 daemon 生命周期 + 插件/审计/hook 同意/沙箱调试/集群节点，支持 JSON 输出与 IPC 客户端。
- **状态**：✅ 已实现。
- **打磨话术**：「加 CLI 子命令在 `src/bin/aleph-server/commands/`；‘CLI 写操作如何不与 daemon 抢锁’走 with_policy（见 CLAUDE.md 进程管理）。」

---

## 6. UI / Panel

### 6.1 流式回显与工作区面板 (Streaming Echo & Workspace Panel)
- **口语关键词**：流式回显、工作区面板、activity timeline、Split 布局
- **代码锚点**：`interfaces/webchat/src/views/chat/messages.rs`（流式 echo，去 card chrome 留纯文本）、`interfaces/webchat/src/components/workspace_panel.rs`（WorkspacePanel + ActivityTimeline）、`src/gateway/event_emitter/types.rs`（StreamEvent）
- **职责**：Panel 两布局——ChatOnly（单列）/ Split（左聊天右工作区）；Split 下工作区按 iteration 显示活动卡（narrative + 工具调用，可展开看 args/result）。
- **状态**：✅ 已实现。
- **打磨话术**：「‘流式回显气泡’在 `views/chat/messages.rs`；‘右侧工作区时间线’在 `components/workspace_panel.rs`。这是前端 Leptos/WASM，改完要重编 binary（rust_embed，见 CLAUDE.md Panel↔Daemon 嵌入链）。」

### 6.2 Panel 双层权限 (Panel Permission Tiers) ⚠️❌
- **口语关键词**：panel 双层权限、对话权限/配置权限、配对 tier、远程连接权限、devices.set_level
- **代码锚点**：前端 `interfaces/webchat/src/components/permission.rs`（ConfigGate / PermissionBanner）、`interfaces/webchat/src/context.rs`（role_is_operator，只认 "operator" 字面量）；后端 `src/gateway/handlers/connect.rs`（LAN-trust 硬编码 `"role":"operator"`）、`src/gateway/method_authz.rs`（OPERATOR_TOOLS 列表）、`src/gateway/caller_identity.rs`、`src/gateway/security/store/devices.rs`、`src/gateway/pairing_store.rs`
- **职责（设计意图）**：Panel 远程 = channel 身份，复用 2 层权限（第一层 = 对话 + 默认工作目录；第二层 = 配置权限 + 自由建工作目录）。
- **状态**：⚠️❌ **现状与设计意图严重偏离——核心结论：LAN-trust 下 2 层权限名存实亡**。逐条核对你的 4 条对齐项：

| 对齐项 | 你的判断 | 查证结论 | 证据 |
|--------|---------|---------|------|
| ① Panel 远程 = channel 身份复用 2 层 | ✅后端已落地 | ⚠️**已变更**：不是分配 tier，而是 LAN-trust 给**所有连接（含 Panel）硬编码 operator** | `handlers/connect.rs` 报 `"role":"operator"`；CALLER_ROLE 默认 Some("operator") |
| ② 第二层 = Panel 全部配置页（含 LLM 配置），前端按 tier 适配 | 后端闸口已通，前端存疑 | ⚠️**前端真有适配，后端名存实亡**：ConfigGate 前端按 is_operator() 锁页，但后端 LAN-trust 下所有 caller 都是 operator → 闸门恒通过 | 前端 `permission.rs:12` ConfigGate；后端 `scoped/dispatch.rs` tool_requires_operator 检查在 LAN-trust 下恒真 |
| ③ 配对后默认只给第一层 (Chat tier) | ✅远程默认 Chat | ❌**不存在默认 tier 机制**：pairing 无 tier 选择，device 表只有 role+scopes，**无 tier/level 字段** | `security/store/types.rs` DeviceRow 无 tier 列；`handlers/connect.rs` 默认即 operator |
| ④ 第二层可配对时选 / 事后 devices.set_level 授权 | 事后有 set_level，配对选 tier 存疑 | ❌**两者都不存在**：SecurityStore 有 upsert/revoke 但**无 set_level/set_tier**；pairing approve 只取 channel+code **无 tier 参数** | `security/store/devices.rs` 无 set_level；`pairing_store.rs` approve 无 tier |

- **打磨话术**：「**重要：你描述的 2 层权限模型在后端基本未实现（LAN-trust 绝对化，全员 operator）。** 前端 ConfigGate 锁是‘诚实投影’但后端不强制。device 表没有 tier 字段、没有 `set_level`、配对不能选 tier。所以——
  - 若你想‘打磨现有 2 层权限’：**先认清现状是 1 层（LAN-trust 全 operator）**，前端锁只是 UI。
  - 若你想‘真正启用 2 层权限’：这是**新功能**，成本 = device_tier 字段 + pairing UI 选 tier + dispatcher 权限检查真生效 + admin set_level API（架构现状刻意按 LAN=信任边界简化，优先级为零）。描述时按‘恢复/新建双层权限’对待，别按‘微调已有’。」

---

## 附录 A. 实现现状体检（⚠️/❌ 清单——打磨时最该先看）

| # | 功能 | 状态 | 现状 vs 直觉的差距 | 若要"做成描述的样子"的性质 |
|---|------|------|---------------------|----------------------------|
| 1 | doctor+f LLM 修复 | ✅ G1 已实现 2026-06-16 | Panel `f` 入口已加（带编辑焦点护栏）；「LLM 修复」= 注入 prompt 走现有 loop+工具（doctor 后端零改动，结构化 findings 早已喂 LLM） | ~~新功能~~ 已完成 |
| 2 | Panel 双层权限 | ⚠️❌ | LAN-trust 全员 operator，前端锁名存实亡 | **恢复/新建双层**（非微调） |
| 3 | 配对时选 tier / devices.set_level | ❌ | device 表无 tier 字段，无 set_level API | **新功能** |
| 4 | ~~kimi vs claude 差异化压缩阈值~~ | ✅ **G4 已实现 2026-06-16** | 窗口比例自动浮动 **+** `[[context_budget.model_thresholds]]` per-model 阈值覆盖（matcher=model id/provider key 子串，逐项回退全局，过防御闸） | **新增配置完成**，见 §2.2 |
| 5 | DAG 工具执行 | ✅ G5 已澄清 2026-06-16 | 工具层=资源群分并行（`concurrency.rs`，非真 DAG）；真任务 DAG 在 `workflow/compile.rs`+`teams/dispatcher/` | ~~描述时分清~~ 已在 §3.3 / §4.3 / §4.4 / 术语表四处区分；**仅澄清，无需开发** |
| 6 | ~~错误沉淀教训(三支柱③)~~ | ✅ **G6 已查证 2026-06-16** | 端到端已连且存活（flag_user_correction→FeedbackDistill→feedback note→召回）；auto error-hook 故意不做(R7/R10) | **零代码完成**，见 §2.5③ |

## 附录 B. 高频"混称"对照（说清楚指哪个）

- **"语音"**：context 注入侧（`thinker/layers/voice_mode.rs`）≠ 运行时 ASR/TTS（`gateway/voice/`）。
- **"DAG"**：工具并发群分（`tools/concurrency.rs`）≠ 任务依赖图（`workflow/compile.rs` + `teams/dispatcher/`）。
- **"权限"**：approval 三级引擎（`src/approval/`）≠ Panel 的 operator tier（`gateway` LAN-trust，§6.2）≠ sandbox 命令策略（`src/sandbox/command_policy/`）。
- **"hook"**：stop hook ≠ sandbox hook ≠ extension shell-hook consent（§5.10 三套）。
- **"命令/工具/斜杠命令"**：同一套 ToolCatalog（§3.5），不是三套。
- **"插件"**：plugins ⊂ extension（`src/extension/`），不是独立目录。
- **"loop vs cron"**：loop 内存态随会话消亡（`src/looping/`）；要持久周期任务用 cron（`src/tasks/cron/`）。
</content>
</invoke>
