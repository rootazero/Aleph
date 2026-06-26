# FEATURE_LOCATOR.md — 功能定位词典

> **用途**：把"打磨某个功能时的口语关键词"翻译成**代码规范名 + 文件锚点 + 精准话术**，让 Claude 在局部优化时一次定位到正确的模块/文件，而不是从头摸索。
>
> **架构主轴**：Aleph agent 按 **Prompt → Context → Harness → Loop** 四层构建。本词典按这四层 + **横切关注点** + **UI/Panel** + **Desktop（桌面端）** 三个补充区组织。
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
| Context | 做梦策略卡死 / wasted distillation 误报 / mutation gate / skill_recall_rate | Dream Signal Integrity & Mutation Gate | `src/memory/dreaming/{signals,mutation_gate,selector}.rs` + `mod.rs::compute_raw_metrics` | ✅ recall 连线(§2.8, 2026-06-26) |
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
| Loop | goal 命令 | Standing Goal | `src/goal/` + `src/builtin_tools/goal.rs` | ✅ |
| Loop | loop 命令 | Loop Command | `src/looping/` + `src/builtin_tools/loop_manage.rs` | ✅ |
| Loop | workflow 命令 | Workflow | `src/workflow/` | ✅ |
| Loop | task 任务管理 / 分解 / 验证 / 收尾 | Coordinated Tasks | `src/agents/swarm/tasks/` + `src/teams/dispatcher/` | ✅ |
| Loop | multi-agent / teams / 多代理 | Teams / Multi-Agent | `src/teams/` + `src/agents/` | ✅ |
| Loop | agent 切换 | Agent Switching | `src/builtin_tools/agent_manage/` | ✅ |
| Loop | 消息流 / 最终结果汇总打印 | Message Stream & Final Answer | `src/gateway/event_emitter/` | ✅ |
| Loop | 新消息排队 / 插入 / 改需求打断 | Message Queue & Steering | `src/gateway/inbound_router/busy_queue.rs` + `execution_engine/steering.rs` + `lane.rs` | ✅ |
| 横切 | 安全模块 | Security Primitives | `src/security/` + `src/pii/` | ✅ |
| 横切 | 全局/agent/channel 三级权限 | Permission Hierarchy | `src/approval/` | ✅ |
| 横切 | LLM 与用户互动 / 确认 / 授权 | LLM-User Interaction | `src/clarification/` + `src/builtin_tools/ask_user.rs` + `src/exec/manager.rs`(授权) | ✅ |
| 横切 | 预设 provider/model / 别名 / 成本路由 | Provider & Model Catalog | `src/providers/presets/` `model_catalog/` | ✅ |
| 横切 | gateway 集群 | Cluster | `src/cluster/` | ✅ |
| 横切 | channel 与 webchat 多端同步 | Channel Sync | `src/gateway/channel_registry.rs` | ✅ |
| 横切 | 打字机模式 / 即时输出全局开关 | Output Mode | `src/config/types/general.rs` + `event_emitter/instant_buffer.rs` | ✅ |
| 横切 | self 自我管理 | Self-Config / Self-Manage | `src/builtin_tools/self_config.rs` `self_manage.rs` | ✅ |
| 横切 | doctor / doctor+f | Doctor & Auto-Fix | `src/builtin_tools/doctor.rs` + `interfaces/webchat/src/state/hotkey.rs`(`f`) | ✅ (G1 已实现 2026-06-16) |
| 横切 | hook | Hook System | `src/verification/stop_hooks.rs` `src/sandbox/hooks.rs` | ✅ |
| 横切 | CLI | Command Line Interface | `src/bin/aleph-server/commands/` | ✅ |
| UI | 流式回显 / 工作区面板 | Streaming Echo & Workspace Panel | `interfaces/webchat/src/components/workspace_panel.rs` | ✅ |
| UI | panel 远程连接 / Gateway token 授权 / QR 配对 / 网页式登录 | Remote Panel Connect & Gateway-Token Auth | `src/gateway/handlers/connect.rs`(`connect_authorized` 纯函数) · `src/gateway/server/handler.rs`(connect 校验+回填+登录墙) · `src/gateway/security/shared_token.rs`(`SharedTokenManager`) · `src/bin/aleph-server/commands/bootstrap_token.rs`(CLI) · `interfaces/webchat/src/{context.rs(token 握手/持久化/scrub),components/token_wall.rs(登录墙),views/settings/security/gateway_token.rs(QR/rotate)}` | ✅ 单层(§6.2) |
| Desktop | 大脑四肢分离 / 能力 trait / Swift 桥 / IPC | Desktop Capability Contracts & Bridge IPC | `desktop/shared/`(`aleph-desktop`：`traits/` + `platform.rs` + `bridge/`) | ✅ |
| Desktop | macOS/Windows/Linux 原生实现 / Swift bridge / 四肢 | Native Bridge Implementations | `desktop/{macos,windows,linux}/src/` + `desktop/macos/bridge/Sources/AlephBridge` | ✅ |
| Desktop | screenshot / 点击 / GUI 自动化 / set-of-marks / ax / 视觉定位 | Desktop Control & GUI Tools | `src/builtin_tools/desktop/` | ✅ |
| Desktop | 通知 / 剪贴板 / 启动/重启应用 / AppleScript / 相机录音 / 备忘录 / Mail / 权限引导 | System / Automation / Permission / Media / PIM Tools | `src/builtin_tools/{system_tool,automation_tool,permission_tool,media_tool,pim}` | ✅ 连线补全(§7.4, 2026-06-17) |
| Desktop | 桌面 App / Tauri / 托盘 / daemon 生命周期 / auto-update / 唤起热键 / 连远程 core | Desktop Shell | `desktop/shell/` | ✅ |
| Desktop | 桌面能力注入 / per-OS 构造 / power inhibit / presence / mic / 平台 OCR | Core Wiring & Daemon Consumers | `src/executor/builtin_registry/builder/constructor.rs` · `src/harness/deps.rs` · `src/tasks/{presence,mic_level}` · `src/vision/providers/platform_ocr.rs` | ✅ |

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
- **代码锚点**：`src/context/compact/compactor.rs`（三策略：LlmSummary / DeterministicTruncation / SessionMemoryReuse）、`src/context/compact/session_split.rs`（压缩失败后 split 新 epoch）、`src/context/budget/mod.rs`（ContextPressure 计算 + `ContextBudgetConfig::preventive_floor`）、`src/context/budget/pressure.rs`（内容感知 token 估计）、`src/context/budget/cheap_passes/`（preflight 廉价 pass：file_op_supersede / tool_result_pruning / image_stripping）；**tool_result_pruning 的内容类型路由见 §2.7**
- **职责**：消息历史压力超 warning 阈值时走侧信道 LLM 摘要，保留 fresh_tail（最近 ~6 条），失败回退确定性截断或旧 summary，极限时 split 新 session。**升级阶梯（preventive band，2026-06-19）**：`< floor`（=`warning − 0.10`）保留全部历史；`[floor, warning)` 触发确定性廉价 pass（无 LLM 成本）；`≥ warning` 触发侧信道 LLM 摘要。三个廉价 pass 的压力门控统一由 `PreflightPipeline::with_min_pressure_ratio(cfg.preventive_floor())` 派生，不再各自硬编码——上下文有余量时不做有损丢弃（headroom 的 `live_zone_only` 压力自适应激进度映射）。
- **状态**：✅ 已实现，三层降级 + 缓存复用 + 零 API 成本路径 + preventive band 升级阶梯。
- **打磨话术**：「‘记忆有效传递又控 token’的核心在 `compactor.rs` 的三策略降级；‘何时触发’在 `budget/pressure.rs` 的阈值 + `preventive_floor` 的廉价 pass 预备带；廉价 pass 门控连线在 `runner_impl.rs` 构造 `PreflightPipeline` 处。」

### 2.2 按模型窗口的压缩时机 (Model-Aware Compaction Timing)
- **口语关键词**：kimi 20 万 vs claude 100 万、不同模型不同压缩时机、模型窗口差异、压缩阈值、per-model 阈值
- **代码锚点**：`src/context/budget/mod.rs`（ContextPressure.compute 按 `token_budget` 参数化）、`src/context/budget/pressure.rs`（按 ratio 动态调整）；预算尺寸 + per-model 阈值在 `src/orchestrator/deps_builder.rs::build_context_budget_config`（`derive_chain_min_budget` 取链上最小窗口模型）；config 类型 `src/config/types/phase6_wiring.rs`（`ContextBudgetToml.model_thresholds` + `ModelThresholdToml` + `threshold_override_for`）。
- **职责**：按当前模型的 token_budget 计算压力比，warning/critical 阈值**相对该预算**触发，100 万窗口比 20 万窗口更晚触发；**且**可按模型覆盖 warning/critical 触发分数；**且**压缩保留的"近期原文尾巴"（`fresh_tail_count`）也随窗口缩放——宽窗口保留更多近期对话原文。
- **状态**：✅ **已实现（G4，2026-06-16）**——窗口尺寸自动浮动 **+** per-model 专属阈值映射 **+**（2026-06-26 增强）窗口感知近期保留：`window_aware_fresh_tail` 让 `fresh_tail_count` 随 usable 预算线性增长（200k→6 锚定向后兼容，1M→~12，上限 16），对齐参考项目 openclaw/pi 的 `keepRecentTokens` 与 hermes 的 `tail_token_budget` 的"按窗口缩放保留"思路（Aleph 用消息计数映射其 token 预算意图）。单一来源 `build_context_budget_config::fresh_tail_count` 同时驱动 preflight 廉价 pass 与 compactor。`[[context_budget.model_thresholds]]` 按"模型 id 或 provider key 的大小写不敏感子串"首匹配覆盖，未匹配/未配置字段逐项回退全局再回退内置 0.70/0.85（向后兼容）；解析后的阈值过同一 `0 < warning < critical ≤ 1.0` 防御闸（坏配置禁用而非降级）。阈值 key 在决定预算的链上最小窗口模型，二者自洽。
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
- 锚点：`src/memory/notes/note/mod.rs`（frontmatter aliases/keywords）、`src/memory/store/sqlite/notes/store_impl.rs`（`resolve_target` / `relink_unresolved`：`[[wikilink]]` 解析 **filename 优先，无命中回退 frontmatter `aliases` 精确匹配**，2026-06-19 连线；alias→path 查询在 `notes/helpers.rs::resolve_paths_by_alias`，JSON1-free serde 精确匹配，`notes_index.aliases_json` 列由 `migrate_notes_index_aliases` 迁移；四信号相关性读 API `related_peers`/`community_peers` 也在此 impl，源表 `notes_graph_related`/`notes_graph_cache`）、`src/memory/notes/graph/relevance.rs`（四信号打分：直接链接 ×3 / IDF 衰减来源重叠 ×4 / Adamic-Adar 共同邻居 ×1.5 / 类型亲和 ×1）、`src/memory/notes/graph/mod.rs`（community detection）、`src/memory/dreaming/stages/graph_recompute.rs`（每周期 materialize 四信号 top-K + community + insights）、`src/memory/dreaming/stages/note_weave.rs`（孤儿回填引擎，三信号，见下）
- 话术：「记忆链接地基 = 笔记 frontmatter 的 aliases/keywords + Note Graph 四信号相关性 + ingest 时自动 peer 链接。**`aliases` 现已参与链接解析**：`[[别名]]` 会解析到以该别名声明的笔记（filename 仍优先）；改解析逻辑去 `store_impl.rs::resolve_target`，alias 查询在 `helpers.rs::resolve_paths_by_alias`。」
- **孤立 note 回填 (NoteWeave 三信号，2026-06-26 增强)**：`src/memory/dreaming/stages/note_weave.rs`——孤立 note（零出零入链接）经三条互补信号回填进链接图：①**关键词重叠**（强信号，`keyword_linker::pair_by_overlap`，无 LLM）——孤儿现在喂**实际正文**（`DreamContext::load_content` + `strip_frontmatter` 去 frontmatter + 截 `BODY_SUMMARY_CHARS=800` 字作 summary）给 `extract_keywords`，关键词反映**内容**而非文件名；②**嵌入最近邻兜底**（`semantic_orphan_links`：关键词配不上的孤儿走 `NoteStore::get_embedding` + `vector_search` 取语义最近邻，**相对距离门控** `SEMANTIC_REL_FACTOR=1.25`（vec0 是 L2 未归一化，绝对阈值不可靠），每孤儿至多 `SEMANTIC_MAX_PER_ORPHAN=2` 边，relation=`"semantic"`，零 LLM）；③**Phase 5 结构兜底**（关键词与嵌入都配不上、但同源的孤儿用 `related_peers` 四信号相关性连到最强结构邻居，阈值 `STRUCTURAL_WEAVE_MIN_SCORE=2.0`，relation=`"related"`，零 LLM——消费 `GraphRecomputeStage` 本周期已 materialize 的 `notes_graph_related`）。合并去重（无序键，关键词边优先）+ `MAX_WEAVE_PER_CYCLE=10` 上限。挂在 Consolidate path `GraphRecomputeStage` 之后（消费 `isolated` insight）、`NoteDecayStage` 之前（新边即时计入 link_weight，破"孤儿→无链接权重→早archived"恶性循环）。
- 话术：「‘孤立 note 连不上网络’＝看 `note_weave.rs`，三信号兜底依次：文本关键词（孤儿正文喂 `build_extraction_input` 带 body）→ 嵌入语义最近邻（`semantic_orphan_links`，复用 sqlite-vec，相对门控不依赖绝对距离）→ 四信号结构相关（Phase 5 `related_peers`，连同源但关键词/语义都不重叠的孤儿，`STRUCTURAL_WEAVE_MIN_SCORE`）。四信号打分（`relevance.rs`）是**纯结构**的（直链/源重叠/共同邻居/类型），此前只喂 retrieval（`note_retrieval`），2026-06-26 才接进孤儿连接路径——改连线逻辑别忘了 `graph_recompute` 必须在 `note_weave` 之前同周期跑（Consolidate pipeline 顺序）；孤儿造边后下个周期结构层自然接管——所以孤儿救援在**造边层** weave 修，不动打分层。」

**② 会话结束实时 flush (Session-End Flush)** ✅
- 锚点：`src/memory/flush/mod.rs`（非阻塞 spawn `session_end_flush`）、`src/memory/flush/registry.rs`（FlushRegistry + await_ready）、`src/memory/compression/mod.rs`（compress_to_notes）
- 话术：「会话结束 flush = 非阻塞 spawn + FlushRegistry，让后续 session 可 await_ready，不阻塞当前 session end。」

**③ 纠正/教训即时沉淀 (Correction & Lesson Sedimentation)** ✅（G6 已查证 2026-06-16）
- **写入**：`src/builtin_tools/flag_user_correction.rs`（LLM 调的工具，写 `RawMemorySource::Correction` 到 `aleph://correction/{id}`）；构造于 `src/executor/builtin_registry/builder/constructor.rs:1793`（**有 `memory_db` 即注册，非死代码**），prompt 引导在 `src/thinker/layers/special_actions.rs`。
- **蒸馏**：`src/memory/dreaming/stages/feedback_distill.rs`（按 `aleph://correction/` 前缀 + watermark 幂等读 → LLM 蒸馏成 `feedback/` note），调度于 `src/memory/dreaming/mod.rs:172,218`（**Consolidate 每日 + Synthesize 两条 dream path 都挂**）。
- **召回**：`feedback/` note 由 assembler 表面化（`src/memory/assembler/gather.rs:284` / `envelope.rs:34`）；goal 教训另有 `GoalLessonsPromoteStage` → `goal-lessons/` note（类别 `goal-lessons`，已补入 indexer `CATEGORY_DIRS`）。
- **治理可见性 (2026-06-20 连线)**：raw correction → distillation 生命周期现经 `memory.list_corrections`（只读，`src/gateway/handlers/memory.rs::handle_list_corrections`）暴露给 panel（Settings ▸ Memory「Corrections」区，`interfaces/webchat/src/views/settings/memory.rs::CorrectionsPanel`）。**纯只读**——写入/蒸馏仍 LLM/工具驱动（守上文设计边界）。
- **状态**：✅ 端到端已连且生产存活（写入工具注册 + distill 双路调度 + 召回消费者，逐跳有单测）。
- **设计边界（重要）**：沉淀是 **LLM/工具驱动**（R8 工具即一切 / R7 LLM 主权）——LLM 判断"这值得记"才调 `flag_user_correction`。**没有也不应有**"每次工具失败自动写 raw memory"的 harness 错误 hook（违 R10「不做错误恢复」+ R7，且会用瞬时报错噪声淹没记忆）。
- 话术：「‘错误/纠正沉淀’走 `flag_user_correction` + `FeedbackDistill`，已全连且存活。想要‘自动捕获工具失败 → 教训’——**这是故意不做的设计边界**（R7/R10），别加 harness 错误 hook；要让 LLM 多记教训就强化 prompt 引导它调工具。」

### 2.6 做梦洞察可见性 (Dream Insights Visibility)
- **口语关键词**：做梦、dream insights、每日摘要、synthesis 笔记、做梦运行历史
- **代码锚点**：`src/memory/store/mod.rs`（`DreamStore::recent_daily_insights` trait 声明）、`src/memory/store/sqlite/sessions.rs`（impl + daily_insights 查询）、`src/memory/dreaming/mod.rs`（`DailyInsight` 结构体）、`src/gateway/handlers/dreaming.rs::handle_list_insights`（RPC 入口）、`interfaces/webchat/src/views/settings/memory.rs::DreamInsightsPanel`（前端展示）
- **职责**：暴露做梦子系统的日报摘要、synthesis 笔记列表、做梦运行历史给 panel 展示。
- **状态**：✅ 已实现（2026-06-20 连线）——`dreaming.list_insights` RPC（只读，复用 `DreamStore::recent_daily_insights` + `NoteStore::list_notes` 过滤 synthesis + `recent_dream_reports`）经 `src/gateway/handlers/dreaming.rs::handle_list_insights` 注册并暴露给 panel Settings ▸ Memory「Dream Insights」区。
- **打磨话术**：「做梦日报在 panel 可见经 `dreaming.list_insights` RPC（`src/gateway/handlers/dreaming.rs`）；前端组件在 `interfaces/webchat/src/views/settings/memory.rs::DreamInsightsPanel`；数据源在 `DreamStore::recent_daily_insights`（trait 在 `src/memory/store/mod.rs`，impl 在 `src/memory/store/sqlite/sessions.rs`）。」

### 2.7 内容类型路由压缩 (Content-Type-Aware Tool-Result Reduction)
- **口语关键词**：按内容类型压缩、ContentRouter、结构化压缩、日志/grep/diff 缩减、tool_result 智能裁剪、headroom 路由
- **代码锚点**：`src/context/budget/cheap_passes/structured/`——`mod.rs`（`ContentKind` 枚举 + `classify()` 分类 + `reduce()` dispatch + `Reduction::render` 诚实头部 + 共享 `render_selected`/`is_error_signal`）、`log.rs`（命令/构建/测试日志：保留 head/tail/error+context，burst 去重）、`search.rs`（grep/rg `path:line:content`：每文件首末匹配 + error 加权，**携带 headroom Rust-port 修复**：Windows 盘符冒号 + 带横杠文件名解析）、`diff.rs`（unified diff：保留 `+/-` 行 + 头部，裁剪上下文到 ±2 行）、`json.rs`（JSON 文档/数组：serde_json `Value` 结构化缩减——长字符串叶子头截断 `…(+N chars)`、大数组保留前 N 元素 + `…(+M more items)`、保留全部 object 键即 `error/status/message` 等短标量原样存活、深度有界防爆栈；用项目自带 serde_json 非新依赖，类型安全优于参考项目的字符串手术）；连线点 `src/context/budget/cheap_passes/tool_result_pruning.rs`（`structured::reduce` 优先，prose/不识别回退首行 placeholder）。
- **职责**：把 headroom 的核心洞见「按内容类型路由到专用压缩器」落地为**确定性廉价 pass**——stale tool_result 不再一律砍成首行，而是按类型保留**信号**（error/summary 行、首末匹配、`+/-` 改动）。零 LLM、零新依赖（纯 Rust 行处理，不引 tree-sitter/Magika/regex 引擎，守 R3/R10）；只作用于 fresh-tail 之外、压力 ≥ `preventive_floor` 时；最终 token 守卫保证永不增长上下文（structured 不更小则回退首行）。
- **状态**：✅ 已实现——log/search/diff/**json** 四类缩减器（json 2026-06-26 接入，classify 顺序 diff→json→search→log，json 门控=首末非空行 `{`/`[` … `}`/`]` 包裹 + serde_json 真解析，解析失败/无超长叶子/反而变大均回退首行）。**仍未做**：headroom `SmartCrusher` 的有损行选择启发式（dedup/variance/Kneedle 自适应采样），当前 json 缩减走结构化 `Value` 因子化已覆盖绝大多数 agent JSON 工具结果；如需更激进采样再扩展。headroom 参考实现在 `/Volumes/TBU4/Github/headroom/headroom/transforms/{smart_crusher,adaptive_sizer}.py` + `crates/headroom-core`（Rust SmartCrusher 后端）。
- **打磨话术**：「‘按类型压缩 tool_result’落在 `cheap_passes/structured/`，分类→dispatch；要新增类型（如 JSON）加一个 `structured/<kind>.rs` + 接 `classify()`/`reduce()`，不改其它缩减器（OCP）。它是 §2.1 preventive band 廉价 pass 里 `tool_result_pruning` 的智能化升级，不是新子系统。可检索压缩（CCR）的 Aleph 对等物是 `ContentIndex`(FTS5) + `result_store` + `ctx_search` + `[Full output persisted]` 标记，见检索/工具层。」

### 2.8 做梦信号完整性与变异闸 (Dream Signal Integrity & Mutation Gate)
- **口语关键词**：做梦策略选择、为什么一直 conserve、自我进化卡住、wasted distillation 误报、skill_recall_rate、mutation gate、震荡检测、防修复死循环、evolver signal 去重对等物
- **代码锚点**：`src/memory/dreaming/signals.rs`（`RawMetrics` 原始指标 + `SignalSnapshot::from_metrics` 归一化四类信号：Health/Recall/SkillUsage/Quality）、`src/memory/dreaming/mod.rs::compute_raw_metrics`（**信号采集单一真源**——folds notes 计数/24h 增长 + 24h 工具调用聚合 + **recall_signals 批量查询**）、`src/memory/dreaming/mutation_gate.rs`（`MutationGate` 三检测器：merge cycle / synthesis oscillation / wasted distillation + cooldown）、`src/memory/dreaming/selector.rs::select`（`growth_pressure = growth_rate*(1-skill_recall_rate)` 驱动策略）、`src/memory/dreaming/evolution/score.rs`（`memory_health_score` 含 skill_recall_rate）、`src/memory/store/sqlite/notes/store_impl.rs::recall_hit_counts`（批量 path→hit_count，仅返回有信号的 path＝召回子集）
- **职责**：`MutationGate` 是 evolver「signal 去重防修复死循环」的 Aleph 对等物——检测合并震荡 / synthesis 自我否定 / 蒸馏空转，命中即 `Conserve` 降级 + cooldown。其有效性**完全依赖** `compute_raw_metrics` 喂的信号是真值。
- **状态**：✅ **信号连线修复（2026-06-26）**——`compute_raw_metrics` 此前**从不查 `recall_signals` 表**（注释自陈是待办 follow-up），`skill_notes_recalled/total`、`never_recalled_count`、`note_hit_rate` 四字段**结构性恒零**，逐跳污染真实消费者：① `MutationGate::detect_wasted_distillation` 的 `distill_history` 仅在产出 skill 时入队、recalled 恒 0 → 5 周期后 `rate=0<0.1` **必误报 → 强制 Conserve**，自我进化恰在正常产出时被 churn 检测器误杀；② `selector` 的 `growth_pressure` 恒判 skill 从不被用 → synthesize 压力畸形最大化；③ `memory_health_score` 被压低。修复＝单次 `recall_hit_counts(all_paths)` 批量查询折叠进 `compute_raw_metrics`（参数 `&dyn RawMemoryStore`→`&SqliteMemoryBackend`，调用点本就传具体类型零改动），后端失败优雅降级为零（字节兼容旧行为）。单测 `compute_raw_metrics_folds_in_recall_signals`。**仍恒零（已知、低价值）**：`duplication_rate`/`contradiction_rate`/`staleness_rate`/`correction_count`/`session_count` 需各自专门聚合查询；当前未接，Health/Quality 信号部分读零——按需再连。
- **打磨话术**：「‘做梦为什么一直 conserve / 自我进化卡住’＝先看 `compute_raw_metrics` 喂给 `SignalSnapshot` 的信号是不是真值；`MutationGate` 设计完备但**信号假零会让它误判**（2026-06-26 已修 recall 三信号）。`gate.evaluate()` 的 `Conserve` 决策驱动 `selector.select`，强制走只跑 lint/review/index/graph 的 conserve pipeline。要‘给某检测器调灵敏度’改 `mutation_gate.rs` 的 `DISTILL_MIN_RECALL_RATE`/`MERGE_CYCLE_THRESHOLD` 等常量；**别在 gate 里补信号**，信号一律从 `compute_raw_metrics` 单一真源接。recalled 是 snapshot、produced 是 per-cycle delta，故 wasted-distillation 实际语义＝‘产出 skill 但召回快照恒 0’才 fire（正确的定性行为）。」

---

## 3. Harness 层

### 3.1 Harness 架构 (Harness Architecture)
- **口语关键词**：harness 架构、Think→Act、笨循环、薄 harness、调度骨架
- **代码锚点**：`src/harness/`——`mod.rs`（8 导出）、`agent.rs`、`agent/think.rs`（LLM 调用+守卫+验证）、`agent/act.rs`（工具执行+并行）、`agent/guardrails.rs`、`agent/prompt.rs`（逐轮消息组装）、`deps.rs`、`trait_def.rs`（Harness trait + TurnState）、`callback.rs`、`chain_context.rs`、`trace.rs`、`trace_sink.rs`
- **职责**：驱动 Think→Act 轮次，管预算/守卫/验证；零意图分类、零工具过滤、零完成度判断（交 LLM + prompt，R7/R9/R10）。
- **状态**：✅ 已实现，受 **CLAUDE.md R10 红线**约束（限 12 文件 / ~4900 行预算）。改这里前先回答"加代码前必答 3 问"。**Gap 分析（vs openclaw/hermes/pi，2026-06-26）**：Aleph 在多维度**已超越**三家参考——`act.rs` 资源域分群并行（三家均 whole-batch 决策）+ `FuturesOrdered` 保序、批内/跨批失败去重、`name_repair` 工具名修复、文本→native 工具调用提升、cooperative steer checkpoint + follow-up watermark（Pi parity）、grace turn×7、verifier 链（Halt/Veto）、反应式压缩 + empty/max_tokens/ctx-overflow 有界恢复。hermes 的"胖 harness"重试矩阵（invalid-tool/JSON/empty/length 多级重试）**有意不移植**——违 R10 第 5 不（不做错误恢复策略选择）。**修复（2026-06-26）**：`think.rs` 流式门控曾按 `guardrails.is_none()` 判定（任意 guardrail 注册即关实时流式），与自身注释契约「no *output* guardrail」矛盾——只 output guardrail 需最终全文。已抽出 `may_stream_deltas()` 按 `output_count()>0` 门控，恢复 input/tool-call-only 部署的实时流式。**重构 + 修复（2026-06-26）**：`think.rs` 的三段恢复（empty / ctx-overflow / max_tokens）原为**固定顺序** while-loop——3b 的 `max_output_tokens` resume 重试会 append partial+nudge 后再发，**该更大请求可自身撑爆 context window**，产生的 `ContextWindowExceeded` 响应漏过早已跑完的 3b-pre 漏出为降级终turn。已把 ctx-overflow drain 抽成有界、幂等的 `drain_context_overflow()`（受同一 one-shot reactive-compact cap 约束），在 3b 前后**两个溢出点**各调用一次，消除顺序脆弱性（熵减）。映射 hermes `conversation_loop.py` 的 finish_reason 单循环分派。
- **打磨话术**：「harness 只管调度，**不要往里加推理逻辑**（违 R10）。要改‘循环行为’找 `agent/think.rs`/`act.rs`；要改‘轮次状态机’找 `trait_def.rs` 的 TurnState。‘guardrailed 部署没有实时流式’＝看 `think.rs::may_stream_deltas`，只有 output guardrail 才该关流式，input/tool-call guardrail 不影响。‘想加 hermes 式多级重试’＝**别加**（违 R10 第 5 不），有界 provider-failure-mode 恢复已在 `think.rs` 3a/3b。」

### 3.2 Tool Calling 2.0 / Tool Use
- **口语关键词**：工具调用、native tool call、并行、结果缓存、result store
- **代码锚点**：`src/harness/agent/act.rs`（执行管道：缓存、并行分组、失败处理）、`src/tools/scoped/`（ScopedToolService：权限/确认/hook/result store）、`src/tools/scoped/dispatch.rs`（pre/post hook + 溢出持久化）、`src/providers/adapter.rs`（NativeToolCall）、`src/tools/runtime.rs`（LoopTool trait）、`src/builtin_tools/file_ops/read_cache.rs`（跨轮未变更重读去重）
- **职责**：LLM 发原生 tool_call → harness 分批并/序执行 → 缓存重复调用 → 超大结果持久化 → 回 ToolResult/ToolError 事件。
- **状态**：✅ 已实现，三层管道（act 分组并行 / ScopedToolService 拦截 / ToolResultStore 溢出）。**重读去重（2026-06-26）**：`file_read` 结果故意不持久化（避免 read-marker loop），原先对"文件没变还重读同一窗口"无保护 → 浪费 context + 可能 read loop。新增 `read_cache.rs`：按 `(canonical_path, offset, limit)` 键，比对 `(mtime, size)`，未变更重读返回紧凑 stub（省去渲染编号正文），二次重读升级措辞；任何变更或 stat 失败一律 fail-open 全量读。机械判定（R7/P8 安全）、落工具层不进 harness（R10 安全）。借鉴 hermes-agent 同模式。
- **打磨话术**：「工具执行三层：`act.rs`(并行保序) → `scoped/`(权限确认 hook) → result store(溢出)。改‘工具结果太大被截断’找 result store；改‘确认弹窗’找 scoped；改‘同一文件被反复重读浪费 context / read loop’找 `file_ops/read_cache.rs`（mtime+size 未变更 → stub，**绝不**改成内容哈希比对那是另一种成本）。」

### 3.3 工具并发调度 (Tool Concurrency)
- **口语关键词**：DAG 工具执行、并行分组、智能调度、资源作用域、并发安全
- **代码锚点**：`src/tools/concurrency.rs`（partition_parallel_groups + ConcurrencyClaim：Shared / Exclusive{Paths/Global}）、`src/tools/runtime.rs`（LoopTool::concurrency_claim）、`src/tools/adapters/registry_adapter.rs`（`READ_ONLY_TOOLS` 只读允许名单 + `file_ops_claim`/`bounded_file_writer_path` 路径作用域）、`src/builtin_tools/file_ops/tool.rs`（按操作类型声明 claim）
- **职责**：工具按资源作用域声明，harness 群分保证无冲突资源并行。
- **状态**：✅ **已澄清（G5，2026-06-16）**——工具层是"群分顺序"而非完整 DAG：按资源作用域分群（`Shared` / `Exclusive{Global, Paths}`）、群内并行群间串行，**没有完整依赖图解析**。`concurrency.rs` 头部自述是"a data-race guard, not an LLM judgement … this only schedules them"（守 R7/R10：不做意图推理/相关性评分/工具过滤）。"智能调度"= 资源冲突避免，**不是**任务 DAG。**真正的任务级 DAG 在**：§4.3 Workflow（`src/workflow/compile.rs`：`step.depends_on → coord_task.blocked_by`，拓扑序物化）/ §4.4 Task（`src/teams/dispatcher/` 按 `blocked_by` 边扫描 Runnable 并发调度）。**安全默认修复（2026-06-26）**：`registry_adapter.rs` 原 `EXCLUSIVE_TOOLS`（变更工具**拒绝**名单，默认 `Shared`＝并行）是**默认不安全**——任何漏列的变更工具静默判为可并行（实测漏 `team_disband`〔且 confirmation-required〕/`team_member_add`/`team_member_remove`/`heartbeat_update`/`skill_install` 等数十个，可与同批调用竞争）。已**倒置为 `READ_ONLY_TOOLS` 只读允许名单**（对齐 hermes `_PARALLEL_SAFE_TOOLS`）：仅显式确证只读的工具 → `Shared`，其余一切（含已知变更工具与未来新增工具）→ `Exclusive{Global}`。失败模式从"漏列变更工具＝竞争"翻转为"漏列只读工具＝串行〔仍正确，只损失并行〕"；拼写错误也只致串行。路径作用域（`file_ops`/`file_write`/`file_edit`/`apply_patch` → 绑定 `Paths`，不相交路径仍并行）保留——这是 Aleph 在安全默认之上**超越** hermes 的点。顺带把浏览器/会话等共享可变状态工具降为安全串行。
- **打磨话术**：「‘DAG 工具执行’在工具层其实是 `concurrency.rs` 的资源群分并行（非真 DAG）。要‘多步骤依赖图’去 Workflow/Task 层（`compile.rs`+`teams/dispatcher/`），**别在 `concurrency.rs` 找、也别在工具层重造 DAG**（违 R6；该需求应上升到 Workflow 层表达）。‘某工具该不该并行’＝看 `registry_adapter.rs::READ_ONLY_TOOLS`——**只读才进名单，默认串行安全**；新增工具默认安全（漏列只损失并行不致竞争），**绝不**改回 mutating 拒绝名单（默认不安全）。」

### 3.4 内置文件工具 (Builtin File Tools)
- **口语关键词**：built-in 工具、read/write file、edit、apply patch、读图片、文件操作
- **代码锚点**：`src/builtin_tools/file_ops/`——`tool.rs`（FileOps 9 operation dispatch：list/move/copy/delete/mkdir/search/stats/batch_move/organize）、`read.rs`（窗口化 cat -n + 二进制嗅探 + 读图）、`image_read.rs`（图片→视觉模型 payload）、`read_cache.rs`（跨轮未变更重读去重）、`write.rs`、`edit.rs` + `edit_match.rs`（exact→排印折叠→CRLF→行锚定模糊→诊断）、`apply_patch.rs`（V4A 多文件结构化补丁，复用 edit_match）、`path_utils.rs`（check_path 黑名单 + 符号链接规范化 + FsScope rebase）、`ops.rs`/`batch.rs`/`search.rs`/`stats.rs`/`text.rs`；`src/builtin_tools/bash_exec.rs`、`src/builtin_tools/generation/`（图像/语音）
- **职责**：文件读写/编辑/补丁 + bash + 生成类原子工具，impl AlephTool 自动生成 schema。
- **状态**：✅ 已实现，路径黑名单 + 沙箱 policy 双守卫；原子写 + 进程级 per-path 锁 + UTF-8 安全窗口读。**Gap 分析（vs openclaw/hermes/pi，2026-06-26）**：file_ops 整体**已超越**三家——编辑模糊匹配移植 codex `seek_sequence` 且叠加排印折叠/CRLF/行锚定诊断、`apply_patch` 用 ~200 行状态机复用 edit_match（codex 是 4600 行 Lark）、`read_cache` 对齐 hermes 重读去重、凭据黑名单广度超 OpenSquilla `sensitive_paths.py` 且加固 Aleph 自身 vault/data。**唯一缺口（pi `read.ts` parity）：`file_read` 读不到图片**——旧实现对一切二进制（含 PNG/JPEG）一律回「not displayable」存根，视觉模型看不见图。**修复 + 连线（2026-06-26）**：新增 `image_read.rs`（仅用既有 `image` png/jpeg/gif/tiff + `base64` 依赖，零新依赖/零平台 API/零 harness 改动）——magic-bytes 嗅探→decode→超 1568px 降采样（Anthropic 最优边长，比 pi 盲缩 2000² 更省 token）→规范化重编码 PNG/JPEG→base64；`read.rs` 的 `is_binary` 分支先试 `encode_for_model`，命中则在 `FileReadOutput` 填 `image_base64`/`format`（serde skip），由**既有** `tools/result_processing.rs::hoist_inline_images` 自动提升为 `ContentBlock::Image`（与 desktop screenshot 同一管线，capability_gate 兜底非视觉模型，image_stripping 管历史图）。
- **打磨话术**：「内置 file 工具都在 `file_ops/`，每个操作一个文件。路径安全在 `path_utils.rs::check_and_resolve_path`。‘读图片让模型看见’＝`read.rs` 二进制分支调 `image_read::encode_for_model`，回 `{image_base64,format}` 后由 `result_processing::hoist_inline_images` 提升为图块——**别在 harness 里加图像处理**（违 R10），复用 screenshot 那条线。‘编辑匹配不上’去 `edit_match.rs`，‘多文件补丁’去 `apply_patch.rs`，‘同文件反复重读’去 `read_cache.rs`。」

### 3.5 工具注册机制 (Tool Registry) & 统一工具/斜杠命令
- **口语关键词**：内置命令注册、工具注册、统一注册、斜杠命令、slash command、热加载
- **代码锚点**：`src/tools/registry.rs`（ToolHandlerRegistry，ArcSwap 无锁热加载 + subscribe 变更广播）、`src/tools/runtime.rs`（LoopToolRegistry，gateway 主表）、`src/command/`（parser.rs 解析 `/input`，types.rs CommandNode 扁平命名空间 + source_type）、`src/tool_metadata/`（ToolCatalog 聚合 builtin/MCP/skill/prompt）
- **职责**：双层注册表（harness 内层 ToolHandlerRegistry + gateway 层 LoopToolRegistry）；斜杠命令解析**委托同一个 ToolCatalog**，命令与工具**同源不双维护**（R8 工具即一切）。
- **状态**：✅ 已实现。**关键事实**：'内置命令注册'、'统一工具注册'、'斜杠命令'在代码里是**同一套**（ToolCatalog 单源），不是三套独立系统。**连线修复（2026-06-26）**：① canonical id 丢失——`resolve_command` 拿到完整 `tool.id`（`mcp:srv:tool`/`plugin:id:name`/`custom:idx:name`），但 `ParsedCommand` 只留裸 `command_name`，下游 `command.execute` 用 `format!("{source}:{name}")` 重建 → 对 MCP/plugin/skill/custom **丢 server/plugin-id/rule-index**。已给 `ParsedCommand` 加 `tool_id`（= `resolved.tool.id`），`handle_execute.internal_id` 与 `serialize_parsed_command` Custom 分支改用之（连线优先，零重建）。② 命名空间分解永远失效——canonical 名用 `_` 分隔（`session_new`），`handle_execute` 却 `split_once('.')` → `namespace`/`action` **恒 None**，与文档响应形 `{"namespace":"session","action":"new"}` 矛盾。新增 `split_namespace_action()` 复用 `TOOL_NAMESPACES`（与 `build_command_tree` 同一分组规则）。**已知缺口**：MCP 工具**不进** ToolCatalog（boot 仅注册 builtin/skill/plugin/custom，无运行时 MCP→catalog 同步）——MCP 斜杠命令经 agent loop 兜底执行，但 `commands.list`/`tools.catalog` 不展示 MCP 工具（发现性缺口，未闭合）。
- **打磨话术**：「斜杠命令 = 工具（同源 ToolCatalog）。加新斜杠命令 = 注册新工具，不要另起命令树。热加载在 `registry.rs` 的 ArcSwap。`command.execute` 返回的 `internal_id` = `ParsedCommand.tool_id`（canonical 注册 id，别再 `{source}:{name}` 重建）；`namespace`/`action` 经 `split_namespace_action`（认 `TOOL_NAMESPACES`，**不是** `.` 分隔）。」

### 3.6 AI 动态路由 (LLM-Driven Routing)
- **口语关键词**：AI 动态路由、意图路由、工具选择、语义路由
- **代码锚点**：`src/harness/agent/prompt.rs`（把工具 schema 列表注入 system prompt）、`src/harness/agent/think.rs`（`.with_tools()` 发给 LLM）、`src/builtin_tools/gateway_route.rs`（**纯确定性 channel→agent 解析查询**，不分类意图）、`src/routing/resolve.rs`（`resolve_route` 层级匹配引擎）
- **职责**：把全部可用工具 schema 注入提示词，**由 LLM 自由选择/组合**；系统不做确定性意图分类或工具过滤。`gateway_route` 只回答"这条消息按 channel/peer 绑定路由到哪个 agent/session"，是配置驱动的 I/O 查询，不碰语义。
- **状态**：✅ 已实现（LLM 主权 R7）。**连线修复（2026-06-26）**：`gateway_route` 工具此前在 `executor/builtin_registry/builder/constructor/mod.rs` 经 `GatewayRouteTool::default()` 注册（空 bindings + 默认 session config）——**无论真实 `[routing]` 配了什么 binding/dm_scope，LLM 调 `gateway_route` 恒回 `main`/`default`**，与真实网关分发（`agent_resolver.rs` 用 `subsystems.rs::with_route_bindings` 快照的真实 bindings）完全脱节，工具向模型撒谎（违 R8 配置应可真实查询）。已改为构造时从**同一份 live `config.config`** 读取 `.bindings`+`.session` 注入 `GatewayRouteTool::new`，与网关同源同默认（`DEFAULT_AGENT_ID="main"`）；取快照而非 live-read 以与同样 boot 快照的网关保持 parity（避免 reload 后再次发散）。
- **打磨话术**：「‘动态路由’= LLM 看全量工具自选（`prompt.rs` 注入）。**不要加规则引擎式意图分类**（违 R7）。`gateway_route` 是确定性 channel 解析，不是意图分类器；它反映**已配置的 `[routing]` bindings**（构造时从 live config 快照，连线点在 `constructor/mod.rs` 的 `gateway_route_tool:` 字段，**别再退回 `::default()`** 那会让工具对真实路由失明）。**已熵减（2026-06-17）**：删除寄生的 regex 任务分类器（旧 `routing/rules.rs` + `routing/task_router.rs` + `tool_metadata` 的 L1/L2/L3 `RoutingLayer` + 死配置 `[task_routing]`）——它们是 Dispatcher 解散遗骸、suggestion-only 无消费者、直接违 R7/P8，已连根清除。」

### 3.7 Shell/Bash 工具 (Shell Execution)
- **口语关键词**：bash、shell、脚本执行、后台进程、wait/poll/kill、后台进程上限
- **代码锚点**：`src/builtin_tools/bash_exec.rs`（BashExecTool + spawn_background + handle_process_action）、`src/builtin_tools/process_registry.rs`（后台进程表：register/poll/**wait**/kill/list + 每会话运行上限 + 完成 Notify）、`src/builtin_tools/code_exec.rs`（通用执行器）、`src/sandbox/workspace.rs`（执行环境）
- **职责**：沙箱隔离的 shell 执行，支持多行脚本、后台进程（poll/**wait**/kill/list）、超时；后台进程**每会话至多 8 个运行中**（`MAX_RUNNING_PER_SESSION`），超限拒绝并引导 poll/kill。
- **状态**：✅ 已实现。**后台增强（2026-06-17）**：① `process_action:"wait"` 用 Tokio `Notify` 阻塞等待完成（非忙轮询，默认 60s 上限 170s，回到前台 180s 预算内）；② 每会话运行中进程上限（修复 `evict_if_needed` 只淘汰已完成条目 → 运行态可无界增长的资源泄漏）。
- **打磨话术**：「bash 工具本体在 `bash_exec.rs`；后台进程生命周期/上限/wait 在 `process_registry.rs`；‘命令安不安全’是另一回事，见 §3.8 沙箱策略。要调后台并发上限改 `MAX_RUNNING_PER_SESSION`；要调 wait 窗口改 `WAIT_DEFAULT/MAX_TIMEOUT_SECS`。」

### 3.8 沙箱命令策略 (Sandbox Command Policy)
- **口语关键词**：sandbox shell 安全、命令过滤、危险命令、hardline、反混淆、policy
- **代码锚点**：`src/sandbox/command_policy/`（mod.rs 引擎、rules.rs 规则集、normalize.rs 反混淆）、`src/sandbox/scrub.rs`（输出秘密清理）、`src/sandbox/hooks.rs`（SandboxBeforeHook 集成）、`src/sandbox/policy.rs`、`exec_approval/`、`deny_globs.rs`
- **职责**：OS 沙箱之前的**内容层**防御：正则硬过滤，分 hardline（不可绕过：fork-bomb/dd/mkfs/rm --no-preserve-root/`rm -rf /`·`/*` 整盘擦除/wipefs·blkdiscard·shred 设备擦除/Windows 灾难形）与 tunable（block/warn/off 三态）；命令先 normalize 反混淆（零宽符/反斜杠/脱字符/反引号/空引号）。
- **状态**：✅ 已实现（2026-06-17 强化：① 修复设备类绕过——`dd`/`>` redirect 漏 `/dev/xvd*`(AWS EC2 根盘)·`dm-`·`md`·`pmem`·`sr`·`loop`，统一并补齐；② 新增 hardline `device_wipe_tools`(wipefs/blkdiscard/shred→/dev/)；③ 新增 tunable warn `shell_eval_download`(`bash <(curl…)` 进程替换 + `eval "$(curl…)"` 绕 `pipe_to_shell`)）。**2026-06-26 强化（对标 hermes-agent）**：① 修复 `rm_rf_system_path` 绕过——旧规则要求 `rf` 在同一 flag token，`rm -r -f /etc`(拆分)/`rm -r /etc`(仅递归，非交互 shell 无确认直删) 可逃逸，改为只需递归 flag(`-r`/`--recursive`/组合) + 绝对系统路径；② 新增 hardline `rm_rf_root`——裸 `/` 或 `/*` 递归删除(busybox/Alpine `rm -rf /` 无 `--preserve-root` 保护、GNU `rm -rf /*` glob 绕过 `--preserve-root`，旧 `rm_no_preserve_root` 漏掉)；③ 新增 tunable warn `system_shutdown`(关机/重启 host：Unix shutdown/reboot/poweroff/init 0|6/systemctl + Win shutdown /s|/r/Stop-Computer/Restart-Computer)、`sudo_privilege_stdin`(`sudo -S/--stdin/--askpass/-s` 猜密码/提权)、`write_ssh_authorized_keys`(写 `~/.ssh/authorized_keys` SSH 后门)）。
- **打磨话术**：「改‘命令拦截规则’找 `command_policy/rules.rs`；‘灾难性底线’在 hardline_rules（即便关 enforcement 也生效，含 `rm_rf_root` 整盘擦除）；可逆但高危的(关机/sudo/ssh 后门)是 tunable warn(`default_rules`)只审计不拦；‘绕过手法’防御在 `normalize.rs`。新规则对标 hermes-agent 的 HARDLINE/DANGEROUS 两层，但守 R7 不抄它的 LLM 智能批准。」

### 3.9 MCP 集成 (MCP Integration)
- **口语关键词**：MCP、外部 server、tools/resources/prompts、OAuth、sampling
- **代码锚点**：`src/mcp/`——`client.rs`（连接）、`manager/`（生命周期）、`transport`（Stdio/Http/Sse）、`tool_bridge`（动态注册 MCP 工具）、`resources`、`prompts`、`approval.rs`、`context_injector.rs`、`auth/`、`external/`、`preflight.rs`
- **职责**：标准 MCP 协议联接外部 server，发现并代理 tools/resources/prompts，支持 OAuth/采样/工具过滤/上下文注入/风险批准。
- **状态**：✅ 已实现（2026-06-17 强化：补齐 MCP **cursor 分页**——`tools/list`·`resources/list`·`prompts/list` 经 `connection.rs::drain_paginated` 跟随 `nextCursor` 翻页直到耗尽，`MAX_PAGES=100` 防呆/防非终止游标；首页不带 `params` 向后兼容旧 server。**修复**：此前三个 list 仅发单次请求，工具数超单页上限的大型 server 会静默丢条目）。**握手强化（2026-06-26，对标 hermes `mcp_tool.py`）**：① **声明 `sampling` 客户端能力**——`protocol.rs::ClientCapabilities` 新增 `sampling` 字段（空对象 `{}`），`aleph_default()` 无条件声明。此前每条连接（`manager/actor.rs`）都已 `set_sampling_callback` 接到 LLM，但握手从不声明该能力 → 守规范的 server 永不发 `sampling/createMessage`，整个已连线的采样子系统对其形同死代码。② **协议版本协商**——`MCP_PROTOCOL_VERSION` `2024-11-05`→`2025-03-26`（与已实现的 Streamable HTTP transport + audio 内容对齐，旧值自相矛盾）；新增 `McpTransport::set_protocol_version` 钩子，`connection.rs::initialize` 把 server 协商回的版本回写，HTTP transport 后续请求据此发 `MCP-Protocol-Version` 头（降级时不再撒谎），非头部 transport 为 no-op。③ **连线 `max_failures`**——`HealthCheckConfig.max_failures`（默认 3）此前是死字段，`ServerHealth::record_failure` 硬编码阈值 5；现已连线为 unhealthy 阈值（重启节奏不变，`should_restart` 对 Degraded 同样触发）。**熵减**：删除死字段 `restart_delay`（其重启 sleep 早已为保证 actor 循环非阻塞而移除，零 reader）。
- **打磨话术**：「MCP 全在 `src/mcp/`；‘MCP 工具如何进 Aleph 工具表’找 tool_bridge；‘外部 server 配置’找 `external/`；‘大型 server 只看到部分工具/资源’= 分页，看 `connection.rs::drain_paginated`（result 类型的 `next_cursor` 在 `protocol.rs`）。‘server 不发 sampling 请求’= 检查握手是否声明 `sampling` 能力（`protocol.rs::ClientCapabilities`，2026-06-26 连线）；‘协议版本/降级’看 `MCP_PROTOCOL_VERSION` + `McpTransport::set_protocol_version`（HTTP 回写协商版本）；‘server 多少次失败算 unhealthy’= `HealthCheckConfig.max_failures` → `record_failure`。」

### 3.10 插件系统 (Plugin System)
- **口语关键词**：plugins、插件、WASM 插件、MCP 插件、marketplace、plugin.json
- **代码锚点**：`src/extension/`——`loader.rs`、`plugin_ops.rs`、`discovery/`、`manifest/`、`hooks/`、`marketplace/`、`capability.rs`、`types/plugins.rs`
- **职责**：管理 Wasm/Mcp/Static 三类插件的发现/加载/注册，多源优先级（Config > Workspace > Global > Bundled）、热重载、风险扫描、marketplace 安装。
- **状态**：✅ 已实现。**关键事实**：'plugins' 在代码里属于 **`src/extension/`**（plugin 是 extension 的一种 kind），不是独立 `src/plugins/`。**硬化（2026-06-17）**：① WASM hook 执行已连线——`loader.rs::execute_hook` 不再返回 "not yet implemented"，而是复用 WASM runtime 的 `call_tool`（hook = 导出函数），`execute_plugin_hook` 补齐 auto-load 与 `call_plugin_tool` 对称；② marketplace 完整性校验 `installer.rs::verify_plugin_integrity` 修复静默跳过——walk 错误 / `strip_prefix` 失败现在硬失败（此前 `filter_map(ok)` 会把不可读文件排除出哈希 → 篡改归档可绕过校验）。
- **缺口已闭合（2026-06-19）**：此前插件注册的 hook 经 `sync_hooks_from_registry` 进 `HookExecutor` 时 `actions` 为空、`handler` 只写不读——**事件驱动的插件 hook 不会从真实 hook 事件触发**（旧描述误判为"需 executor 持有 loader 回调、跨模块循环依赖"）。**修复（连线优先 + 依赖倒置 P4）**：新增 `HookAction::Plugin{plugin_id,handler}` 变体；`sync_hooks_from_registry` 改发该 live action（替代空列表，`handler` 字段保留供 `validation.rs` 显示）；`executor.rs::execute_plugin` 经**既有全局访问器** `try_extension_manager()`（gateway/channel fire-site 已在用）调 `execute_plugin_hook` 派发 WASM 导出函数——**复用现成基础设施，零回调注入、零 Arc 所有权环**。派发集中在 `execute_action` 一处，自动覆盖 observer/interceptor/resolver 三态；observer 忽略输出，interceptor/resolver 可经行前缀协议读结构化返回。manager 未注册（单测）时优雅跳过。
- **WASM HTTP 能力连线（2026-06-26）**：整套出站 HTTP 安全栈（`runtime/wasm/allowlist.rs::AllowlistValidator` HTTPS-only+反混淆+路径穿越防护、`capabilities.rs::HttpCapability` 的 rate_limit/请求&响应体大小上限、`capability_kernel.rs::check_http_limit` 调用次数上限）**已建已测但 linker 从未注册 `http_fetch` host function → 整栈零非测试消费者（死代码）**。**修复（连线优先，超越参考项目）**：`mod.rs` linker 追加 `http_fetch` 导入；`host_functions.rs::host_http_fetch` 顺序强制 能力声明→allowlist→调用次数→速率限制→请求体上限→`reqwest::blocking`（带 timeout，**专用 std 线程执行避免嵌套 tokio runtime panic**）→响应体上限，全程返回 `{"status","headers","body"}` / `{"error":...}` JSON 永不 panic；kernel 新增 `check_http_request`（接 `AllowlistValidator`）、`check_rate_limit`（用既有 `RateLimit` 类型做滑窗，使其 LIVE）、`http_config` 访问器。**openclaw/hermes 均无沙箱化插件 HTTP**，Aleph 现做到默认拒绝+HTTPS allowlist+速率/大小封顶。**剩余 seam**：`credential_injector.rs`（宿主侧凭证注入，插件永不见明文）仍未接——需把**异步** secret vault 穿过 loader→runtime→HostState 多层 + async-in-sync 桥接，属跨切面改动，待 secret 解析方案设计后单独连线（当前 `inject_credential` 仍零调用，如实标注，不假连）。
- **打磨话术**：「插件 = extension（`src/extension/`）。三类 kind：Wasm/Mcp/Static。改‘插件优先级/发现’找 discovery，与 Skill 共享优先级解析。‘WASM hook 怎么跑’= `loader.rs::execute_hook` → runtime `call_tool`；‘事件驱动的插件 hook 怎么派发’= `HookAction::Plugin` → `executor.rs::execute_plugin` → `try_extension_manager()` → `execute_plugin_hook`（2026-06-19 连线，见上‘缺口已闭合’）。要加新 hook 事件源只需在 fire-site 调 `fire_global_observer` 即可，插件 hook 自动参与。‘WASM 插件怎么发 HTTP’= host import `http_fetch`（`runtime/wasm/host_functions.rs::host_http_fetch`），策略全在 `capability_kernel.rs`（`check_http_request`/`check_rate_limit`/`http_config`）+ `allowlist.rs`；‘凭证注入’（`credential_injector.rs`）尚未连线（见上 2026-06-26 剩余 seam）。」

### 3.11 技能系统 (Skill System)
- **口语关键词**：skills、技能、SKILL.md、资格评估、prompt 注入、共现
- **代码锚点**：`src/skill/`——`manifest.rs`（SKILL.md 解析）、`registry.rs`、`installer.rs`、`eligibility.rs`、`preprocess.rs`、`prompt.rs`（build_skills_prompt_xml）、`guard.rs`（安全扫描）、`cooccurrence.rs`
- **职责**：解析 SKILL.md → 评估资格 → 执行安装指令 → 注入 prompt → 跟踪使用与共现。
- **状态**：✅ 已实现，与插件共享源优先级（workspace > plugin > global > bundled）。**Prompt 预算连线（2026-06-17）**：`prompt.rs` 的 `SkillPromptBudget` 降级引擎（full→compact 两层 + 省略 note）原先只硬编码默认值（64 skills / 12k chars），**从未连到任何配置**。现已连线：`skills.toml [prompt_budget]`（`max_skills`/`max_chars`，`0`=不限，缺字段回退默认）→ `SkillsConfig.prompt_budget` → `SkillSnapshot.prompt_budget` → `PromptConfig.skill_prompt_budget` → `SkillInstructionsLayer` 用 `build_skills_prompt_xml_with_budget` 渲染权威索引。`snapshot.prompt_xml` 仍是默认预算预览（生产不读，仅测试用），真正注入由层按配置预算渲染。
- **打磨话术**：「技能定义解析在 `manifest.rs`，‘何时把技能塞进 prompt’在 `eligibility.rs` + `prompt.rs`。要调‘prompt 里列几个技能/占多少字符’改 `skills.toml` 的 `[prompt_budget]`（配置项，非代码）；连线终点在 `thinker/layers/skill_instructions.rs`，预算来源在 `skill/config.rs::SkillsConfig.prompt_budget`。」

### 3.12 浏览器自动化 (Browser Automation)
- **口语关键词**：browser、浏览器、screenshot、Chrome MCP、Playwright、网络策略
- **代码锚点**：`src/browser/`——`backend.rs`（BrowserBackend trait）、`chrome_mcp_backend.rs`、`playwright_cli_backend.rs`、`manager.rs`、`network_policy.rs`、`tab_registry.rs`、`secret_guard.rs`、`types.rs`
- **职责**：统一文本优先浏览器接口，双后端（Chrome DevTools MCP / Playwright CLI），截图/点击/导航/填表/JS/网络隔离/凭证过滤。
- **状态**：✅ 已实现。**硬化（2026-06-26）**：① **wait 超时钳制（修 panic）**——`browser_wait_for` 的 `timeout_ms` 此前无界透传两后端：Playwright backend `Instant::now() + Duration::from_millis(timeout_ms)` 在 `u64::MAX` 时**溢出 panic**，Chrome MCP 则把巨值直接喂给 MCP 把 tab 占满整会话；现 `wait_for.rs::clamp_timeout` 在系统边界钳到 `500..=120_000`（对齐 openclaw `resolveActWaitTimeoutMs`，schema doc 同步标注有效区间）；② **截图体积预算（补对称缺口）**——文本读早有 `bound_content` 30k 字符封顶，截图却原始 PNG 字节无界，`full_page` 可数千 px 高 → base64 灌爆模型请求；现 `browser_tools/mod.rs::bound_screenshot_png` 复用 `file_ops/image_read` 已验证的 `image` 0.25 缩放路径，把最长边钳到 `MAX_SCREENSHOT_EDGE=1568`（Anthropic 服务端阈值，超此服务端必缩，多传纯烧 token），保持 PNG 格式契约（vision bridge 仍走 `ImageFormat::Png`），已在预算内/解码失败则**原样返回**（后处理绝不把成功截图变失败）。
- **打磨话术**：「浏览器双后端在 `backend.rs` trait 下；‘换后端/加操作’改对应 *_backend.rs；‘网络隔离’在 `network_policy.rs`；‘凭证 IN/OUT 双向过滤’在 `secret_guard.rs`（URL 扫描 + 内容脱敏），读时再校验在 `browser_tools/mod.rs::make_backend_and_tab_guarded`。**模型可调的两个预算/超时都在边界钳**：‘wait 超时’= `wait_for.rs::clamp_timeout`（500–120000ms），‘截图像素’= `browser_tools/mod.rs::bound_screenshot_png`（最长边 `MAX_SCREENSHOT_EDGE`），与文本读的 `DEFAULT_CONTENT_MAX_CHARS` 三者同源思路。」

---

## 4. Loop 层

### 4.1 Goal 命令 (Standing Goal)
- **口语关键词**：goal 命令、自主目标、持久目标、自动续跑、迭代/token/deadline 上限
- **代码锚点**：`src/goal/`（mod.rs / types.rs / store.rs）、`src/tasks/goal_pursuit.rs`、`src/builtin_tools/goal.rs`（GoalAction: set/get/update/clear/**list**——单文件）
- **职责**：用户设持久目标，LLM 经 goal 工具管状态，后台按 迭代/token/deadline 上限自主续跑，每轮注入进度 lessons + 剩余配额。
- **状态**：✅ 已实现（should_continue / continuation_prompt / cap/deadline/budget_reached_note 全连，门控器决定客观完成）。**list 连线（2026-06-26）**——`get` 只读当前 session（goal 按 session_key 一会话一行），跨会话设的 goal 在别的通道用 `get` 看不见，违 R6 一核多端 + R8 工具即一切（无法自然语言问"我有哪些在跑的目标"）。底层 `GoalStore::list_all()` 早建好（dream lessons-promote 在 sweep）却从未暴露给 LLM；现新增 `GoalAction::List` 分支复用之，紧凑渲染（`render_list_line`，按 `updated_at_ms` 倒序，标注 `(this session)`，pursuit/budget/deadline 同 `render` 词汇），零 schema 漂移、零新增字段。
- **打磨话术**：「goal 状态机在 `src/goal/`；‘续跑触发’在 `tasks/goal_pursuit.rs`；用户面工具在 `builtin_tools/goal.rs`。‘列出所有会话的 goal’= `GoalAction::List`→`GoalStore::list_all()`（跨会话；`get` 只看当前会话）。」

### 4.2 Loop 命令 (Loop Command)
- **口语关键词**：loop 命令、周期循环、定时、cadence、内存态
- **代码锚点**：`src/looping/`（mod.rs / types.rs / pursuit.rs）、`src/builtin_tools/loop_manage.rs`（LoopAction: start/stop/status/update——单文件）
- **职责**：内存 HashMap 维护每会话 LoopState（Fixed/Timeout），hook 按 next_wake 定时触发续跑 RPC。
- **状态**：✅ 已实现（含 fail-closed `stop_loop_on_failure` + update 原地重定速）。**注意**：状态**只存进程内，daemon 重启清零**（设计意图"随会话消亡"）。**硬化（2026-06-17）**：① **停因连线（修死计算）**——cap-reached 分支此前算出 `note`（token/deadline/iteration 三因）后只 `info!` 丢弃；现 `LoopState.stop_reason` 字段存储之，失败路径 `stop_loop_on_failure` 同样写入，`loop(action='status')` 表面化，静默封顶的 watch loop 能在下一轮自报停因；② **`status` 打磨**——从裸 `{:?}` Debug dump 改为 `human_summary()` 人类可读摘要（cadence "every 5m"、ticks N/cap、time left、next wake「in 8m」、停因）；③ **`update`/`stop` 诚实化（修误导）**——对已停 loop 的 `update` 不再谎报 `"Loop updated"`（hook 只对 Active fire，永不再跑），改返回 `success=false` 引导 `start`；`stop` 已停 loop 报 `"already stopped"`；④ **熵减**——三路选 note 的 if/else 下沉为 `pursuit::stop_reason_note()`，execute.rs 一行调用。**硬化（2026-06-26）**：① **剩余配额注入（连线，对齐 goal）**——`pursuit::tick_prompt` 此前只重述 prompt + tick 号，loop 内模型对"还剩几 tick / 还有多久"全盲，无法自我收尾（与 `goal_pursuit::continuation_prompt` 注入「N/max + LAST-step 警告」不对称）。现 `tick_prompt(state, now_ms)` 注入剩余 tick 数（`max - n`）、deadline 剩余时间（`fmt_duration_ms`）、**最后一 tick 收尾警告**（`n >= max` 时换"This is the LAST tick … wrap up"）——智慧在 prompt（R9），让模型自我调度而非确定性无进度检测（R7 不越俎代庖）；② **`update` next_wake 诚实化（修隐性 no-op）**——`next_wake` 用在 Fixed cadence 上时 `tick_delay_ms` 忽略它，旧码却静默存死状态报 `"Loop updated"`（同 2026-06-17 诚实化漏网者）；现 Fixed loop 上 `next_wake` 返回 `success=false` 引导用 `interval`；③ **零封顶边界校验（P7）**——`max_iterations=0` / `timeout_minutes=0` 此前可创建"出生即死" loop（首次 hook 即 exhausted，报困惑的"reached the cap (0 ticks)"），现 `reject_zero_cap` 在 `start`/`update` 边界拒绝。
- **打磨话术**：「loop 状态在 `src/looping/`，**内存态、重启丢失**别当持久。要持久周期任务用 cron（`src/tasks/cron/`）。停因在 `LoopState.stop_reason`（`pursuit::stop_reason_note` 选 token/deadline/iteration 三因），status 输出在 `LoopState::human_summary`，duration 显示走 `types::fmt_duration_ms`（`parse_interval_ms` 的逆）。**续跑注入的剩余配额 + 最后一 tick 收尾警告**在 `pursuit::tick_prompt(state, now_ms)`（对齐 goal 的 `continuation_prompt`）；`next_wake` 只对 model-paced loop 有效（Fixed loop 改速用 `interval`，工具会诚实拒绝 Fixed 上的 `next_wake`）；零封顶由 `loop_manage::reject_zero_cap` 在边界挡掉。」

### 4.3 Workflow 命令 (Workflow)
- **口语关键词**：workflow 命令、DAG 工作流、步骤模板、workflow.js 互转、per-step 模型覆盖、提案评审
- **代码锚点**：`src/workflow/`——`def.rs`（WorkflowDef）、`compile.rs`（materialize → coord_tasks + blocked_by 边；workflow_model_override）、`clarify.rs`（闸门）、`proposal.rs`（import/review/accept）、`store.rs`、`interop/`（.workflow.js）
- **职责**：声明式 WorkflowDef → 编译为 DAG coord_tasks → TeamDispatcher 按拓扑并发；每步可覆盖模型；支持 .workflow.js 无损互转 + 提案审批。
- **状态**：✅ 已实现（per-step model override 经 manifest → RunRequest.model_override，零 harness 侵入 R10）。
- **打磨话术**：「真正的多步骤 DAG 在 `src/workflow/`（不是工具层 concurrency）；‘编译成任务图’在 `compile.rs::materialize`。」

### 4.4 协调任务 (Coordinated Tasks)
- **口语关键词**：task 任务管理、规划、分解、子任务分配、实施、验证、收尾、僵尸任务
- **代码锚点**：`src/agents/swarm/tasks/`（mod.rs 数据模型 / store/ 持久化 / dag.rs 环检测 / acceptance.rs 验收 / **retry.rs 有界重试** / **timeout.rs 每任务超时**）、`src/teams/dispatcher/schedule.rs`（select_schedulable + **fail_or_retry** + is_zombie）、`src/teams/dispatcher/runner.rs`（execute_member_task）、`src/teams/dispatcher/handoff.rs`（build_recovery_section 续做上下文）
- **职责**：DAG 中每个 CoordTask 按 blocked_by 扫描依赖，上游完成→Runnable，分派器选最闲 owner 并发执行；失败/超时**有界自动重试**（默认 2 次=至多 3 次尝试，每次重试携带前序 recovery 上下文续做，且**指数退避**间隔），耗尽预算→FailedFinal，僵尸（worker 失联）→强制失败不重试。
- **状态**：✅ 已实现。**重试连线（2026-06-17）**：此前 `fail_task` 失败即永久 `Failed`，文档承诺的「失败重试 3 次→FailedFinal」是空头——recovery 基础设施（`build_recovery_section`「这是第 N 次尝试」+ 退出日志续做 + `coord_task_runs` 逐次历史）全建好却只能靠 leader 手动 reset 或孤儿回收触发。现新增纯决策 `retry.rs::retry_decision`（有界计数）+ `schedule.rs::fail_or_retry`：失败时数已记录的失败 run 次数对比 `max_retries`（任务 metadata 覆盖，否则 `DispatcherConfig.default_max_retries=2`），未超限 reset `Pending`（**激活既有 recovery 注入**），超限才走 `fail_task`（终态 `Failed`=FailedFinal）。孤儿回收留 `Running` 行不计入预算；僵尸绕过重试直接 `fail_task`。per-task 覆盖经 `task_create` 的 `max_retries` 参数透传。**退避增强（2026-06-17）**：此前 reset `Pending` 后 `run_task` 结尾 `signal()` 当 tick 即重派——transient 失败（限流/过载）几十毫秒内打光重试预算。现 `fail_or_retry` 计算 `retry.rs::jittered_backoff_secs`（指数 `base*2^(n-1)` 封顶 + **equal jitter** `[delay/2,delay]`，`DispatcherConfig.retry_backoff_{base,cap}_secs` = 5s/120s），把 `retry_not_before` 戳进 metadata（复用既有通道，零 schema 漂移），`dispatch_once` 的 `is_retry_eligible` 门在 I/O 边界跳过未到期任务（`select_schedulable` 仍是纯时间无关公平函数）；并 spawn Tokio 精确延时 `signal` 唤醒，短退避不必等 60s fallback tick。jitter seed = task id 哈希（确定性、无 RNG 依赖，但逐任务相异）→ 整团同时失败的任务不再锁步重试再次踩塌恢复中的 provider（thundering herd）。`base=0` 退回即时重试（向后兼容）。**注意**：CoordTaskStatus 实为 10 态（含派生 `Blocked`/`Unsatisfiable`），无独立 `FailedFinal` 状态——`Failed` 即终态，重试期任务回 `Pending` 不落 `Failed`。tasks 无直接用户工具，经 workflow/teams leader 间接驱动。
- **打磨话术**：「任务调度/依赖/僵尸检测在 `teams/dispatcher/`；‘失败重试几次’= 纯函数 `tasks/retry.rs::retry_decision` + 连线 `schedule.rs::fail_or_retry`，调默认改 `DispatcherConfig.default_max_retries`、按任务改 `task_create` 的 `max_retries`；‘重试间隔/退避’= `tasks/retry.rs::backoff_secs` + `DispatcherConfig.retry_backoff_{base,cap}_secs`，门在 `dispatch_once` 的 `is_retry_eligible`（`retry_not_before` metadata）；‘重试时续做而非重来’= `handoff.rs::build_recovery_section`（智慧在 prompt，R9）；任务数据结构在 `agents/swarm/tasks/`。**operator 调参（已连线）**：`[team_dispatcher]` TOML 子表（`default_max_retries`/`retry_backoff_{base,cap}_secs`/`zombie_ttl_secs`/`max_per_owner`/`max_concurrent`/`task_timeout_secs`…，缺字段回退 `DispatcherConfig::default()`，boot 站点 `agent_init/mod.rs` 映射，zombie_ttl 钳到不低于 task_timeout）——此前 dispatcher 恒 `::default()`，文档承诺的可调性是空头。**每任务超时（已连线 2026-06-26）**：`task_timeout_secs` 此前仅全局；现 `task_create` 的 `timeout_secs` 参数经 `timeout.rs::with_task_timeout` 写入 metadata（复用 max_retries 的 metadata-override 范式，零 schema 漂移），`schedule.rs::run_task` 用 `effective_timeout_secs(meta, 全局)` 取覆盖值传 `execute_member_task`，深研究子任务与快步骤不再共用一个 600s TTL（24h 防御上限）。配套修正：僵尸阈值 = `max(全局 zombie_ttl, per-task timeout)`（折进 `is_zombie`），否则超过全局 zombie_ttl 的长任务在进程重启后会在自身 deadline 前被误杀；`reclaim_zombies` 同步收口为调用纯谓词 `is_zombie`（删除内联副本，消除测试/实现漂移）。」

### 4.5 多代理 / 团队 (Teams / Multi-Agent)
- **口语关键词**：multi-agent、teams、多线程多任务多代理、leader、群聊广播、roster
- **代码锚点**：`src/teams/`——`dispatcher/`、`messages/`（路由）、`broadcast/mod.rs`（GroupChatBroadcaster::dispatch，防风暴三道闸 MAX_CHAIN_DEPTH=6 / MAX_FANOUT_WIDTH=5 / **MAX_TOTAL_ACTIVATIONS=32**）、`store.rs`、`workflow_canvas.rs`；`src/agents/`（registry/runtime/subagent_spawner/swarm）
- **职责**：leader 创建团队并分解任务（建 coord_tasks），成员并发执行，消息经 Aggregator 合并后 MessageRouter 投递，群聊可自主链式接话（深度 + 单轮宽度 + 整树累计唤醒三重封顶）。
- **状态**：✅ 已实现。**强化（2026-06-17）**：① 群聊防风暴补齐**全局唤醒闸** `MAX_TOTAL_ACTIVATIONS`——此前仅 depth×width 是局部约束，最坏 `5^6≈1.5万` 次成员 run 可炸开；现整棵 fan-out 树共享一个累计计数器（`Arc<AtomicUsize>` 随 `dispatch_user` 新建、随递归下传），越界跳过且**恰好跨界一次**发系统提示（原子天然去重，不刷屏）；② fan-out join 不再静默吞 `JoinError`——成员任务 panic 降级为 `warn` 可观测；③ 熵减：删除 `dispatcher/acp_bridge.rs` 中从未被调用的 `execute_acp_member_task`（活体执行在 `runner.rs::execute_member_task`，桥接模块现仅保留 `AcpMemberRef` 命名约定解析）。**防风暴闸 operator 化（连线，2026-06-26）**——三道闸 + transcript 预算此前是 4 个裸 `const`（改一个要重编），与同模块 §4.4 已可调的 `DispatcherConfig` 不对称；参考项目（hermes `delegation.*` / openclaw `agent-limits.ts` / pi 常量集中）全把这类上限做成配置。现新增运行时 `BroadcastConfig`（默认值读自这 4 个 const，单一来源零漂移）+ `[team_broadcast]` TOML 子表（`max_chain_depth`/`max_fanout_width`/`max_total_activations`/`transcript_token_budget`，全 `Option`，缺字段回退 `BroadcastConfig::default()`），沿 `app_config.team_broadcast` → boot 映射（`agent_init/mod.rs`，紧邻 `[team_dispatcher]` 映射，零值按 P7 回退默认防"出生即死"群聊）→ `handle_chat_send` → `GroupChatBroadcaster::new` 流入——与 §4.4 的 `[team_dispatcher]`→`DispatcherConfig` 完全同构。纯函数 `over_depth(depth, max)`/`resolve_targets(…, max_fanout_width)`/`format_transcript(…, budget)` 改为接收闸值（保持 host 可测）。**错误连线**：`messages/router.rs` thread-escalation 通知此前 `let _ =` 静默吞失败→改 `tracing::warn!`；`dispatcher/clarify.rs` 锁释放失败→`tracing::debug!`（对齐同文件 park 失败的显式 warn）。**注意**：`[team_broadcast]`（telegram 式 teams 对等群广播）与旧 `[group_chat]`（`src/group_chat/` persona 圆桌：max_personas/max_rounds/coordinator）是**两套独立系统**，勿混称。
- **打磨话术**：「‘多代理协作/群聊’在 `src/teams/`；‘单个 agent 怎么跑/怎么 spawn 子代理’在 `src/agents/`。两者配合。‘群聊会不会被模型乱 @ 炸开’= 三道闸：深度 `MAX_CHAIN_DEPTH`、单轮宽度 `MAX_FANOUT_WIDTH`、整树累计 `MAX_TOTAL_ACTIVATIONS`（默认值在 `broadcast/mod.rs`，**operator 调参在 `[team_broadcast]`**：`max_chain_depth`/`max_fanout_width`/`max_total_activations`/`transcript_token_budget`，boot 站点 `agent_init/mod.rs` 映射，零值回退默认；运行时结构 `teams::broadcast::BroadcastConfig`，与 §4.4 的 `DispatcherConfig` 同构）。ACP 成员执行不在 `acp_bridge.rs`（那只管 `acp:` 命名解析）而在 `runner.rs`。**别混称**：`[team_broadcast]`(§4.5 teams 对等群) ≠ `[group_chat]`(`src/group_chat/` persona 圆桌)。」

### 4.6 Agent 切换 (Agent Switching)
- **口语关键词**：agent 切换、创建/删除/列出、绑定 channel、项目覆盖、agent 配置
- **代码锚点**：`src/builtin_tools/agent_manage/`（create/delete/list/info/**switch**）、`src/agents/registry.rs`、`src/agents/loader.rs`（lookup_with_overlay 项目覆盖）；运行时绑定 `src/gateway/agent_env/manager_ops.rs::set_active_agent`，inbound 消费 `src/gateway/inbound_router/agent_resolver.rs`（Tier2 `get_active_agent`）。
- **职责**：agent 工具管生命周期，全局（~/.aleph/agents/）+ 项目层（project/.aleph/agents/）两级，项目层可影子覆盖全局。**切换** = 把当前 channel 的活跃 agent 重绑到另一个已存在实例。
- **状态**：✅ 已实现。**切换工具连线（2026-06-17）**：新增 `agent_switch` 工具——本节namesake 此前缺失（只有 Panel RPC `channels.set_agent`，LLM 无法经自然语言切换，违 R8）。`switch.rs` 复用 `SessionContext.__channel` 注入链 + 实例 `AgentRegistry`（存在性校验，杜绝绑幽灵 agent）+ `set_active_agent` + `GatewayEventBus`（新 `AgentLifecycleEvent::Bound`），逐点接入 9 处枚举（definitions/groups/method_authz(operator 门控)/registry_adapter(EXCLUSIVE)/slash_command/dispatch）。顺带修 `create.rs` 死引用（旧描述让模型调不存在的 `agent_bind`→ 改 `agent_switch`）。**硬化（2026-06-26）**：① **切换静默失效修复（核心连线）**——`agent_resolver` Tier1（route_bindings 非空）此前提前 return，**永不读 Tier2 workspace 绑定**：配了路由的用户一旦 `agent_switch`，工具报成功但下一条消息仍走旧 agent（namesake 工具变 no-op，违 R8 诚实工具）。现 Tier1 解析为 `MatchedBy::Default`（无具体绑定命中本会话）时，先查 `get_active_agent(channel)` 覆盖，让显式切换生效；具体绑定（Peer/Guild/Team/Account/Channel）仍优先，保留精配路由（override 走 `route=None` 由 `build_context_with_agent` 为覆盖 agent 重建 session key）；② **`agent_list` 连线 `__channel`（兑现 DESCRIPTION）**——工具描述承诺"show which one is currently active for this conversation"，但 `call()` 此前 `_args` 直接丢弃 `__channel`，输出无活跃标记。现读 `__channel`，活跃 agent = `get_active_agent` ?? `registry.default_agent_id()`（镜像 inbound 解析），输出加 `active_agent` + 每行 `active: bool`，Display 标 `→ (active)`，模型可经自然语言知"我现在是哪个 agent"（R8）；③ **多通道绑定展示打磨（修有损）**——`get_all_agent_bindings()` 用 `HashMap<agent_id,channel>` 把 many-to-one 塌缩（绑多通道只显示一个）；新增非破坏 `bindings_by_agent() → HashMap<agent_id, Vec<channel>>`（按 channel 排序），`agent_list` 改用之，`bound_channel: Option` → `bound_channels: Vec`，原 `get_all_agent_bindings` 保留供 Panel `agents.bindings` RPC 不破坏。
- **设计边界**：`agent_info` 查的是 AgentDef 子代理目录（explore/coder…），`list/create/delete/switch` 查运行时实例注册表——两套**刻意分层**（persona 实例 vs 子代理 role），未合并。
- **打磨话术**：「agent 创建/切换/列出/删除 工具在 `agent_manage/`（同一运行时实例注册表）；切换落 `switch.rs`→`set_active_agent`，生效点在 `agent_resolver.rs`（零配置走 Tier2；**配了 route_bindings 时走 Tier1 的 `MatchedBy::Default` 覆盖分支**——具体绑定优先，否则显式切换胜）。‘列表标活跃 agent / 列全部绑定通道’在 `agent_list`（`bindings_by_agent` + `active_agent`/`active`，读注入的 `__channel`）。‘加载与项目覆盖’在 `agents/loader.rs`（那是 AgentDef 侧，另一套）。**生命周期事件投递（已修）**：`AgentLifecycleEvent::{Bound,Deleted,Registered}` 经 `bus.publish_json` 时须包成 `TopicEvent`（带 topic）——裸发是 topic-less，被 WS forwarder 当 topic="" 由 `agent.lifecycle.*` 订阅丢弃（Panel 收不到）。**删除解绑（已修）**：`agent_delete` 用 `clear_bindings_for_agent`（按 agent_id 删全部 channel 绑定），不再只解一个（多对一绑定模型，否则残留幽灵）；`set_active_agent` 改 `INSERT OR REPLACE` 原子 upsert。」

### 4.7 消息流与最终答案汇总 (Message Stream & Final Answer)
- **口语关键词**：对话消息流、StreamEvent、最终结果汇总、final_response、RunComplete、汇总打印输出、instant/打字机缓冲
- **代码锚点**：`src/gateway/event_emitter/`（types.rs StreamEvent、impls.rs `GatewayEventEmitter`、instant_buffer.rs `plan_instant`+`InstantBufferingEmitter`、origin_fanout.rs/team_fanout.rs fan-out 装饰器）；**最终答案净化/提取单一源** `src/gateway/reply_emitter/extract.rs`（原子 `sanitize_final_response()` + 扫日志 `extract_final_response()`）。消费者：`teams/broadcast` 群聊 transcript + `tasks/cron` cron（走 `extract_final_response`）；`origin_fanout`/`team_fanout`/`telegram/streaming/orchestrator`（走原子 `sanitize_final_response`）。
- **职责**：执行中逐步发 StreamEvent（Reasoning/ToolStart/ResponseChunk/RunComplete），`RunComplete.RunSummary.final_response` 是**未净化**的最终文本（源头 `runner_impl.rs` 取 `content.text` 原样）；任何把它**投递到 surface** 的路径都必须经单一源净化（剥 `<think>`/completion 标记，纯 thinking → None 不投递）。instant 模式"流式文本合并成单条"由 `plan_instant` 状态机统一驱动两个 emitter。
- **状态**：✅ 已实现。**熵减（2026-06-17）**：① 最终答案提取从 broadcast+cron **两份漂移副本**收口到 `extract_final_response` 单一源；② instant 缓冲状态机从内联+镜像 **两份**收口到 `plan_instant` sink-无关 planner。**连线修复（2026-06-26）**：抽出原子 `sanitize_final_response`(单字符串→净化 Option)，`extract_final_response` 两分支复用之；**堵住 3 处 fan-out 裸投递泄漏**——`OriginFanoutEmitter`(→origin 渠道 Telegram/Slack)、`TeamFanoutEmitter`(→Panel 群聊气泡 `team.<id>.message`)、`telegram/streaming/orchestrator`(→answer lane finalize) 此前都把**未净化的** `summary.final_response` 直发 surface，绕过 inbound `ReplyEmitter.send_to_channel` 恒净化的对称路径，导致 `<think>` 泄漏 + 实时气泡与持久 transcript 漂移 + 纯 thinking 噪声投递。现三者同走 `sanitize_final_response`。**注意**：**最终答案没有独立的表**，靠扫描 StreamEvent 日志找 RunComplete 抽取。
- **打磨话术**：「‘最后那段汇总输出怎么来的’= harness 发 RunComplete 事件，消费方调 `reply_emitter::extract_final_response()` 扫事件日志。没有‘答案表’。要改‘提取/sanitize/fallback 规则’改 `reply_emitter/extract.rs`（单一源,`sanitize_final_response` 原子 + `extract_final_response` 扫日志,别再在消费者里手写副本）；**任何把 final_response 发到 surface 的新路径都必须经 `sanitize_final_response`**（`final_response` 源头未净化,fan-out 装饰器/Telegram orchestrator 都已连线,别再裸发）；要改‘instant 缓冲/打字机’改 `instant_buffer::plan_instant`（单一源，GatewayEventEmitter 与 InstantBufferingEmitter 都走它）。」

### 4.8 消息排队与改需求打断 (Message Queue & Steering)
- **口语关键词**：新消息排队、插入策略、agent 执行中改需求、打断、插队、steering、lane 优先级、busy-input 策略
- **代码锚点**：
  - **busy 排队**：`src/gateway/inbound_router/busy_queue.rs`（per-agent FIFO + 原子 ticket，上限 32/agent，`REJECT_NEWEST` 溢出）、`src/gateway/inbound_router/executor.rs`（register→is_front 轮询投递，`BUSY_QUEUE_MAX_WAIT_SECS=1800`）
  - **改需求/打断（三态策略）**：`src/gateway/execution_engine/mod.rs`（`BusyInputMode` = Steer/Interrupt/Queue + `BUSY_INPUT_MODE_KEY`）、`src/gateway/execution_engine/execute.rs`（busy 分支按模式分流）、`src/gateway/execution_engine/steering.rs`（mid-loop 注入 + 合并去重 + `MAX_PENDING_STEERING=16` 背压 + teardown rescue）
  - **优先级**：`src/gateway/lane.rs`（Lane 分类 Query/Execute/Mutate/System + ChannelClass 双池 Panel 优先）、`src/gateway/server/handler.rs`（按 loopback 派生 ChannelClass 并 `acquire`）
  - **取消/续跑**：`src/gateway/cancellation.rs`、`src/gateway/resume_coordinator.rs`
- **职责**：agent 繁忙时新消息进 `busy_queue` per-agent FIFO（上限 32，仅队首尝试投递，保到达序，超 30min 才通知失败而非静默丢弃）；同会话有活跃 run 时，按通道 `BusyInputMode` 分流——**Steer**（默认，注入 live event log 让运行中的 loop 下个轮次接住）/ **Interrupt**（取消同会话 sibling，经 busy_queue 以全上下文重启）/ **Queue**（不打扰，排队等当前 run 结束）；Lane + ChannelClass 让本地 Panel 优先于 Bot/CLI。
- **状态**：✅ 已实现。**熵减（2026-06-17）**：删除死代码 `session_scheduler.rs`（631 行，per-session FIFO 旧版调度器，`::new`/`enqueue` 仅存在于自身测试，零生产消费者）——其职责早被 harness `try_start_run` 每-agent 闸 + `busy_queue` FIFO + `steering` 完整取代，违 R10 YAGNI 故连根清除。**Interrupt 空内容门（连线 2026-06-26）**：`Steer` 分支早有空内容门（无 text 无 attachment → 不注入空 `UserMessage`），但 `Interrupt` 分支**无对称防护**——一个内容为空却携带（或继承）`Interrupt` 模式的请求（resume 续跑占位、丢了 `try_start_run` 竞争的 synthetic run）会**白杀**同会话健康 sibling 后又无新指令可贡献，净亏在途工作。此前仅靠 `build_steering_rescue_request` 在单一调用点 `metadata.remove(BUSY_INPUT_MODE_KEY)` 防御（注释明言「inherited Interrupt would cancel that legitimate sibling」=危险已知），属 load-bearing workaround。现把内容谓词单一源化为纯函数 `steering::has_steering_content(request)`（`!input.trim().is_empty() || !attachments.is_empty()`，`Steer` 内联门收口复用之，熵减），并在 `execute.rs` 的 `Interrupt` 决策点内禀门控：空内容直接 `target=None` 跳过 cancel，落 busy-queue 等待——对齐 Hermes `internal`-event 永不打断，未来任何"空内容+Interrupt"自动安全（rescue 的 remove 降为 belt-and-suspenders）。
- **打磨话术**：「‘用户改需求/插队/打断’的真核心 = `busy_queue.rs`（agent 级 FIFO）+ `steering.rs`（mid-loop 注入）+ `BusyInputMode`（Steer/Interrupt/Queue 三态，在 `execution_engine/{mod,execute}.rs`）+ `lane.rs`（Panel 优先）。要改‘改需求时是排队、注入还是打断当前 run’就动 `execute.rs` 的 busy 分支 + 对应通道的 busy-input 模式 + `cancellation.rs`。**注意**：`session_scheduler.rs` 已删除——它从来不是活跃路径，别再去找它。**mid-turn steering 开关（已连线）**：`[execution] mid_turn_steering`（默认 true，命名 default fn 保留旧行为）→ `ExecutionEngineConfig.mid_turn_steering`，此前硬钉 true 无 operator 出口；关闭即回退 legacy busy/retry。**空内容打断保护**：`Steer` 与 `Interrupt` 两分支共用纯谓词 `steering::has_steering_content`——空内容请求既不注入空消息也不杀 sibling（Hermes `internal`-event parity）。」

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
- **代码锚点**：`src/approval/`（policy.rs / types.rs / config.rs / audit.rs / session_route.rs / operator_requester.rs / node_requester.rs / adapters.rs）、`src/config/types/policies/tool_permissions.rs`、`src/gateway/inbound_router/permission.rs`、`src/gateway/channel_policy.rs`。**注**：旧 `src/event/permission.rs` 已删（§5.3 熵减），勿再找。
- **职责**：分级授权——工具级（action_type → block/allow/ask）、通道级（per-channel policy）、代理级（全局默认），支持临时授权记忆与跨节点 escalation。
- **状态**：✅ 已实现（ApprovalPolicy::check → Allow/Deny/Ask）。工具名权限三级合并（global→agent→channel，最严格胜）在 `gateway/execution_engine/run_loop.rs`，强制点在 `tools/scoped/`（Deny 隐藏+拒绝、Ask 走审批门）。**glob 对齐（2026-06-17）**：`config/types/policies/tool_permissions.rs::resolve` 此前**仅精确匹配**工具名，与 action-type 审批层 `ConfigApprovalPolicy` 的 glob 能力不对称；现复用同一 `crate::approval::matches_glob`，override key 支持 `*`/`?` 通配（如 `"mcp__*"="ask"`、`"*_delete"="deny"`）。优先级：精确名 > 多 glob 命中取最严格 > default，与 `merge` 三级合并自洽（精确条目穿透 merge，杜绝 channel/agent 用精确名绕过 global 的 glob deny）。**审计身份单源化（2026-06-26）**：action-type 审批层的 5 个受门控工具（desktop/browser/system/pim/automation）此前各自构造 `ActionRequest` 的问责字段——desktop/browser 各有一份近乎相同的 `audit_identity`（仅 `desktop.`/`browser.` 前缀不同），而 system/pim/automation **硬编码 `agent_id: String::new()`（空）**，使其审批审计记录匿名化，违背本节「支持临时授权记忆」与 §5.1 审计承诺。现统一为单一源 `src/approval/audit.rs::audit_identity(domain, action, target)`（从 `TURN_CONTEXT` 解析问责 agent_id + `"{domain}.{action} ({target}) via {channel}/{conversation}"` 上下文，turn 外回退 `"main"`）；5 工具全部复用，`target` 字段保持原样以不改策略匹配语义。熵减净 −89 行（2 副本 + 3 冗余测试 → 1 源 + 单测）。
- **打磨话术**：「三级权限引擎在 `src/approval/`；‘通道级覆盖’在 `gateway/channel_policy.rs`；‘人工确认/集群上报’在 operator_requester/node_requester。‘工具名权限三级合并’在 `run_loop.rs`，强制在 `tools/scoped/`。**工具名权限要按家族批量配**用 glob（`tool_permissions.rs::resolve`，复用 `approval::matches_glob`，精确名优先）。**审批审计‘谁发起的’问责字段**统一走 `approval::audit_identity`（单源），别再在工具里手搓或留空 `agent_id`。action-type `Ask` 不走确定性 HITL（回吐 `approval_required` 给 LLM 自决，R7/R9），真正的人审门在 §5.3 `ExecApprovalManager`。」

### 5.3 LLM 与用户互动 (LLM-User Interaction)
- **口语关键词**：确认消息、授权、clarification、ask_user、Halo 浮窗、permission request、选项描述
- **代码锚点**：Clarification 侧 `src/clarification/`（mod.rs `ClarificationRequest`/`ClarificationOption` + `with_description`、session.rs `ClarificationManager`/`interpret_reply`）、`src/builtin_tools/ask_user.rs`（`AskUserChoice` = 字符串 | `{label, description}` + `build_choice_keyboard` inline 按钮）；reply 回流 `src/gateway/inbound_router/mod.rs::try_intercept_hitl`（含 `clarify:` 按钮回调分支，孪生 `approve:`）。Permission 侧（真实路径）`src/exec/manager.rs`（`ExecApprovalManager` + `ApprovalDecisionType` AllowOnce/Session/Always/Deny + `clamp_decision` 风险钳制）+ `src/approval/`（§5.2）；slash 解析 `inbound_router::is_slash_command`，Panel 走通知中心按钮（`interfaces/webchat/src/api/exec_approval.rs`）。
- **职责**：双路——Clarification（菜单/文本/**inline 按钮**，可带 per-option 描述）经原通道投递、reply 由 inbound router 拦截解释；Permission（action 确认）经 `ExecApprovalManager` 配对挂起/恢复，均带 timeout。
- **状态**：✅ 已实现。**熵减（2026-06-17）**：删除被取代的旧事件驱动权限/问答死代码 `src/event/permission.rs` + `src/event/question.rs`（`PermissionRequest/Reply/Event`、`QuestionRequest/Reply/Event` 零构造零消费，真实授权用 `ApprovalDecisionType` 而非 `PermissionReply`）+ `AlephEvent`/`EventType` 7 个从未 emit 的变体 + 孤儿 `UserResponse`。**连线（2026-06-17）**：`ask_user` 接入 `AskUserChoice` 结构化选项，打通此前死字段 `ClarificationOption.description`（菜单渲染 `1. label — 描述`，向后兼容 `choices=["a","b"]`）。**细节打磨（2026-06-17）**：`ClarificationManager::register` 注册时机会性清扫过期条目（对齐孪生 `ExecApprovalManager::register_pending`）——修复被中止 run 丢弃的 `ask_user` 留下的孤儿条目泄漏（`cleanup_expired` 此前生产零调度，现经 register 重新可达）。**连线（2026-06-26 · clarify 按钮）**：补齐**孪生不对称**——审批侧早有 Telegram inline 按钮（`approve:<id>:<decision>`，§5.2 `ApprovalBridge`），而 clarification 侧此前**仅纯文本菜单**（对标 hermes-agent `send_clarify` 缺口）。现 `ask_user::build_choice_keyboard` 给 `OutboundMessage.inline_keyboard` 挂选项按钮（callback_data `clarify:<1-based idx>`，2/行，>12 选项退回纯文本，label 超 32 字符截断），`try_intercept_hitl` 新增 `clarify:` 前缀分支：剥前缀后用裸索引 `ClarificationManager::resolve`（复用 `interpret_reply` 数字路径＝等同用户输入数字），陈旧点击静默消费不泄漏 token。**零新模块/管理器/配置**：复用 `InlineKeyboard` + `cb_` 回调短路 + 现有 resolver；不支持按钮的通道仍优雅退回数字文本菜单（对标 hermes base fallback）。
- **打磨话术**：「‘问用户选项/要信息’走 `ask_user` + `src/clarification/`（要带选项说明用对象形 `{label, description}`，连线终点 `ClarificationOption.with_description`；**Telegram 等通道现渲染可点按钮**，callback `clarify:<idx>` 在 `try_intercept_hitl` 剥前缀后 resolve，孪生于审批 `approve:`）；‘要授权确认’走 `src/exec/manager.rs` 的 `ExecApprovalManager` + §5.2 `src/approval/`，**不是** `event/permission.rs`（已删）。reply 回流统一在 `inbound_router::try_intercept_hitl`。」

### 5.4 预设 Provider 与模型路由 (Provider & Model Catalog)
- **口语关键词**：预设 provider、模型别名、规范化、能力门控、成本路由、failover、metadata
- **代码锚点**：`src/providers/presets/registry.rs`（PROFILES 单一源 + 别名展开）、`src/providers/model_catalog/`（alias.rs / endpoint.rs / 能力矩阵）、`src/providers/capability_gate.rs`、`src/providers/failover.rs`、`src/providers/metadata.rs`、`src/pricing.rs`
- **职责**：PROFILES 驱动预设别名（Kimi=Moonshot）；model_catalog 存能力矩阵 + 端点定位（`endpoint.rs` Local/Cloud）；capability_gate 做 per-model 需求匹配；failover + pricing 驱动降级/选型。能力/成本/端点定位三类参考数据统一暴露到 `providers.catalog` RPC（Panel picker）与 `list_models` 工具（LLM 选模，R8 模型可感知）。
- **状态**：✅ 已实现。端点定位 Local/Cloud 已从 route_policy 内部连出到 catalog + list_models（`EndpointKind::as_str`）；`RateCard` 已补全 cache_creation/reasoning 费率投影。**成本路由连线（2026-06-19）**：`LoadBalanceStrategy` 新增 `cost_aware`（对标 LiteLLM `lowest-cost` + RouteLLM 成本轴）——此前 `pricing.rs` 价格表只喂事后成本*估算*，从未参与*选型*。现 `failover/provider.rs::price_hint` 把每候选首模型的 blended `input+output` 费率（`pricing::rate_card`，milli-USD/Mtok）折进 `LoadMetric.price_per_mtok`，`route_policy::balance_group` 的 `CostAware` arm 升序排序，R7 prompt-blind。**未定价模型按 `EndpointTier` 分流（2026-06-26）**：此前“未定价=0=最便宜排最前”会把*未定价的云端*模型（新模型/拼写错误/不在价表里）也当成“免费”排到价表可确认的模型之前——成本未知 ≠ 零。现 `failover/provider.rs::unpriced_cost` 按候选 tier 分流：`Local`=0（自托管确实免费，排最前），`Cloud`/`Unknown`=`u64::MAX`（成本未知→排最后，绝不抢在可确认价格之前，但仍保留在链中可回退）；tier 经 `tier_by_name` 喂进 metric 闭包。`route_status` 快照新增 `price_milli_per_mtok`（排序依据）+ `endpoint_tier`（解释为何某未定价 provider 这样排）；Panel 路由设置页 + `route_config` RPC 已收口该选项。
- **打磨话术**：「加/改预设别名在 `presets/registry.rs`；‘某模型支不支持 vision/tool-use’在 `model_catalog/capabilities.rs` + `capability_gate.rs`；‘本地还是云端模型’在 `model_catalog/endpoint.rs`（已暴露到 catalog/list_models 的 `endpoint` 字段）；‘成本’在 `pricing.rs`（`RateCard` = picker 用的费率投影）；‘按成本路由’= `[route] load_balance = "cost_aware"`（同一层级内最便宜优先，连线点 `failover/provider.rs::price_hint`，sort 在 `route_policy::balance_group`）。」

### 5.5 集群 (Cluster)
- **口语关键词**：gateway 集群、单中心非对称节点、反向 RPC、node_invoke、node_list、扇出、center approval、LAN-trust
- **代码锚点**：核心 `src/cluster/`（mod.rs / reverse_rpc.rs / registry.rs / node_runtime.rs / node_file_cmd.rs / node_approval.rs）；中心侧 LLM 工具 `src/builtin_tools/{node_list,node_invoke,node_invoke_many,node_file}.rs`；中心 RPC `src/gateway/handlers/cluster.rs`（enroll/deregister/environments.list）；节点拨出运行时 `src/bin/aleph-server/commands/node.rs`；文档 `docs/reference/CLUSTER.md`。
- **职责**：单中心 + 边缘节点，节点主动连中心反向 RPC（结构区分请求/响应，断线 `cancel_all` fail-fast）；agent 经 `node_invoke`（单节点，多级寻址 + 歧义枚举）/`node_invoke_many`（按 tag AND 并发扇出）/`node_file`（8MB + sha256 jail 传输）跨边界执行；节点侧 `CommandTable` allowlist 权威；高风险能力升级经 `CenterApprovalRequester` fail-closed 回中心人工 escalation。
- **状态**：✅ **全量实现（含四个 LLM 工具、tag 扇出、文件传输、离线节点合并视图、确定性舰队排序）**。信任模型 = **LAN-trust**：enroll 不铸 token，节点凭 `connect` 帧参数形状（`commands`+`tags`）声明身份，已移除旧的 token/signature/pairing/`run_pairing`/`AUTH_FAILED` 机制（CLUSTER.md 已同步）。**名字寻址归一化对齐（2026-06-27）**：`registry.rs::match_id` 此前精确名**大小写敏感**、模糊仅原样 substring，离线回退 `handlers/cluster.rs::resolve_enrolled_node` 更严（仅大小写敏感精确名）——与所映射的 openclaw `node-match.ts::normalizeNodeKey`（`[^a-z0-9]+ → -`）不对称，导致 "GPU Box" 无法用 "gpu-box" 寻址、且在线/离线两路语义漂移。现新增单一真源 `cluster::normalize_node_key`（转小写 + 标点/空格折叠为 `-` + 去首尾 `-`），在线 `match_id` ②/③ 与离线 `resolve_enrolled_node` 共用：名字匹配大小写 + 标点不敏感，归一化撞键（"Worker 1" vs "worker-1"）按 ② 报歧义。精确 id 仍原样（UUID 内连字符语义不折叠）。
- **打磨话术**：「集群在 `src/cluster/`；‘节点怎么连中心’看 reverse_rpc.rs + node.rs；‘模型怎么驱动节点’看 `builtin_tools/node_*`；‘离线/在线舰队视图’看 `handlers/cluster.rs::handle_environments_list`；信任边界=LAN，无 token、无认证层。**节点按名寻址大小写/标点不敏感**（"GPU Box"≡"gpu-box"），归一化逻辑单一真源 `cluster::normalize_node_key`，在线 `match_id` 与离线 `resolve_enrolled_node` 共用——改名字匹配规则只动这一处。」

### 5.6 多端通道同步 (Channel Sync)
- **口语关键词**：channel 多端同步、webchat 同步、通道注册表、统一消息总线、delivery queue、投递重试调参、发送重试上限
- **代码锚点**：`src/gateway/channel_registry.rs`（中心注册表 + `SendRetryPolicy`/`SendRetryTomlConfig`）、`src/gateway/channel.rs`、`src/gateway/delivery_queue.rs`（durable 队列 + `DeliveryQueueConfig`/`DeliveryQueueTomlConfig`）、`src/gateway/channel_health_monitor.rs`（僵尸通道自重启）、`src/gateway/config.rs`（`[gateway]` 三个弹性子表）、`src/gateway/interfaces/`（telegram/discord/wechat/matrix/signal…）；启动连线 `src/bin/aleph-server/commands/start/builder/subsystems.rs::initialize_channels`。
- **职责**：中心 ChannelRegistry 管所有通道，inbound 统一广播进 event bus（所有 agent 可见），outbound 经 delivery queue（rate-limit retry + 可选 SQLite 持久化）；三个弹性旋钮（健康监控/durable 队列/发送重试）现已全部配置化。
- **状态**：✅ 已实现。**配置连线（2026-06-17）**：此前 `DeliveryQueueConfig` 与 `SendRetryPolicy` 全是硬编码 `::default()`，`with_send_retry_policy` 更是只被测试调用的**生产死接口**（违 R8 连线纪律，与 §2.2 model_thresholds / §3.11 skill prompt_budget 同型缺口）。现新增 `[gateway.delivery_queue]`（attempts/backoff/tick/queue_len）+ `[gateway.send_retry]`（retries/retry_after）两个 TOML 子表，经 seconds 基 `*TomlConfig` → `to_runtime()`/`to_policy()`（含 flooring 防御：tick=0 busyloop、backoff=0 即时重投等坏配置全部 clamp）→ `initialize_channels` 流进 store 构造与 registry，激活死接口。缺字段逐项回退内置默认（旧 TOML byte-identical 兼容）。与既有 `[gateway.channel_health]` 同源对齐。
- **打磨话术**：「‘消息怎么在多端同步’= inbound 广播进 event bus + outbound delivery queue。加新通道在 `gateway/interfaces/`；‘掉线重试/持久化’调参在 `[gateway.delivery_queue]`，‘rate-limit 重试几轮/等多久’在 `[gateway.send_retry]`，‘僵尸通道自重启’在 `[gateway.channel_health]`——**三个都是配置项不是代码改动**，连线终点在 `initialize_channels`。坏配置由 `to_runtime/to_policy` 兜底 clamp，不会 busyloop。」

### 5.7 输出模式：打字机 / 即时 (Output Mode)
- **口语关键词**：打字机模式、流式输出、即时输出、全局开关、所有 channel 同步、output_mode
- **代码锚点**：`src/config/types/general.rs`（BehaviorConfig.output_mode + typing_speed）、`src/gateway/event_emitter/instant_buffer.rs`（instant 装饰器）、`src/gateway/handlers/agent.rs`（resolved_output_mode，每次 run fresh 读）、`src/gateway/inbound_router/executor.rs`（通道路径 fresh 读）、前端 `interfaces/webchat/src/components/markdown.rs`（StreamingRenderer）
- **职责**：全局 `behavior.output_mode` = typewriter（逐字符，可设速度）/ instant（整体返回）；instant 是 EventEmitter 装饰器包裹任意 inner emitter；Panel run 与 inbound channel run **同源**，改配置下次运行即生效无需重启。
- **状态**：✅ 已实现（这是真·全局开关 + 全通道同源，**与你的描述一致**）。
- **打磨话术**：「全局开关在 `config/types/general.rs` 的 output_mode；‘所有 channel 同步’靠 `handlers/agent.rs::resolved_output_mode` 每次 run fresh 读同一配置。前端呈现在 `webchat/.../markdown.rs`。」

### 5.8 自我管理 (Self-Config / Self-Manage)
- **口语关键词**：self 自我管理、自动配置、LLM 驱动配置、配置向导、改完 provider 验证可达
- **代码锚点**：`src/builtin_tools/self_config.rs`（身份文件 + config.toml CRUD + `verify` 探活）、`src/builtin_tools/self_manage.rs`（LLM intent → 读 self SKILL.md 自管理手册）、`src/config/patcher.rs`（`ConfigPatcher`：deep-merge/校验/备份/回滚 + `health_check` provider 探活，`with_vault` 注入金库）、`src/config/reload_impact.rs`（live/restart/inert 分类）、`src/builtin_tools/doctor.rs`
- **职责**：self_manage 读 `~/.aleph/skills/self/SKILL.md` 导航自管理；self_config 交互式配置 + `update_config(verify=true)` 在 patch `providers.*` 后经 `providers::probe::probe_provider`（单一真源）探活回填 `health_check`；密钥走 vault_store，结构改动触发 hot-reload / reload_impact 告知生效时机。
- **状态**：✅ 已实现。**健康探活已连线（2026-06-17）**：`PatchRequest.health_check` 不再是 no-op——命中 `providers.*` 且 patcher 注入了 vault 时，apply 后探活并回填 `Passed/Failed/Skipped`，同源服务 `providers.healthcheck` RPC 与 doctor `providers/connectivity`。死字段 `secret_fields` 已移除（熵减）。
- **打磨话术**：「‘自我管理/自动配置’= self_manage + self_config 两工具；密钥不进 config.toml 走 vault。‘改完 provider 想确认能连’= `update_config(..., verify=true)`（仅 `providers.*` 有效，dry_run 跳过），探活逻辑在 `patcher.rs::run_provider_health_check`，复用 `providers::probe`。」

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
- **代码锚点**：`src/bin/aleph-server/cli.rs`（Clap 定义）、`src/bin/aleph-server/commands/`（mod.rs 分发 + doctor/plugins/audit/hooks/secret/node/sandbox_debug/prompt_size/start）、`src/bin/aleph-server/daemon.rs`（start/stop/**status** 生命周期 + 单例锁交互）、`src/cli/ipc_client.rs`、`src/cli/endpoint.rs`（`.ipc-endpoint.json` 发现文件）
- **职责**：Clap 驱动入口，覆盖 daemon 生命周期 + 插件/审计/hook 同意/沙箱调试/集群节点，支持 JSON 输出与 IPC 客户端。
- **状态**：✅ 已实现。**status 连线强化（2026-06-17）**：① **修缺陷**——PID 文件**只在 `daemonize()`（Unix daemon 路径）写入**，前台 `aleph start`（无 `-d`）与 **Windows** 均不写 → 旧 `status` 对正在运行的前台/Windows server **误报"未运行"**；现 `handle_status` 连线 `.ipc-endpoint.json`（所有启动路径都写，持 live PID+URL+started_at），对 endpoint PID 做存活探测后合并 PID 文件信号，前台/Windows 可见。② **增强表面化**——`status` 现输出 **URL / Uptime / Version**（Uptime 复用 `looping::types::fmt_duration_ms` 渲染，与 loop status 同词汇）；`--json` 增 `url/started_at/uptime_seconds/version` 字段（向后兼容超集，`running/pid` 键保留）。③ **纯函数 `resolve_status`**（注入存活探测 + `now`，可单测）：**live endpoint 胜过陈旧 PID 文件**；陈旧 endpoint（server 崩溃未清理）经存活探测**不**广告死 URL。④ **`stop` 清理 endpoint 文件**——`cleanup_endpoint_file()` 在停止确认/陈旧分支 best-effort `remove_endpoint`，消除前台/Windows 残留。
- **打磨话术**：「加 CLI 子命令在 `src/bin/aleph-server/commands/`；‘CLI 写操作如何不与 daemon 抢锁’走 with_policy（见 CLAUDE.md 进程管理）。‘status 为何对前台/Windows server 失明’= 旧版只读 `gateway.pid`（仅 daemon 写）；现 `daemon.rs::resolve_status` 已连线 endpoint 发现文件。要调‘status 显示什么’改 `StatusReport` + `print_status_human`；uptime 渲染走 `fmt_duration_ms`。」

---

## 6. UI / Panel

### 6.1 流式回显与工作区面板 (Streaming Echo & Workspace Panel)
- **口语关键词**：流式回显、工作区面板、activity timeline、Split 布局
- **代码锚点**：`interfaces/webchat/src/views/chat/messages.rs`（流式 echo，去 card chrome 留纯文本）、`interfaces/webchat/src/components/workspace_panel.rs`（WorkspacePanel + ActivityTimeline）、`interfaces/webchat/src/components/tool_card.rs`（共享工具卡片，聊天侧+工作区侧同源）、`interfaces/webchat/src/state/layout.rs`（`WorkspaceState`：tool_payloads + 展开覆盖集）、`src/gateway/event_emitter/types.rs`（StreamEvent）
- **职责**：Panel 两布局——ChatOnly（单列）/ Split（左聊天右工作区）；Split 下工作区按 iteration 显示活动卡（narrative + 工具调用，可展开看 args/result）。
- **状态**：✅ 已实现。**卡片展开连线（2026-06-17）**：`ToolCard` 的展开/折叠此前用**卡内本地 `RwSignal`**，有两处缺陷——① 渲染 ToolCard 的 keyed `<For>` 因 `row_key` 折入内容长度，**每个流式 token 都 remount**，本地信号被重置回 `default_open` → 流式中用户手动折叠的卡片下一 token 又弹开；② 同一工具被聊天气泡与工作区时间线**两张卡**各自渲染、各持本地信号 → 展开不同步。同时 `WorkspaceState` 早已建好 `expanded_events` + `toggle_event`/`is_event_expanded` 却**零消费者**（死基础设施）。现把展开态连到该共享集（语义改为「相对 kind 默认的覆盖集」：`effective_open = kind.default_open() XOR contains(tool_id)`，default-open 类无需播种；`is_event_expanded`→`is_event_toggled` 正名），一次性修复两个 bug 并激活死代码；storybook 无 `WorkspaceState` 时回退卡内本地信号。顺带把 `StepStrip` 硬编码英文 `step/steps` 改为 i18n（`chat.step`/`chat.steps`，en/zh 对称）。**i18n 补全（2026-06-19）**：team tab（「交付物」/「任务」）+ 空态（「暂无交付物」/「暂无任务」）此前**中文硬编码**（en locale 也显示中文），已接入 leptos-i18n（`common.team_deliverables`/`team_tasks`/`team_no_deliverables`/`team_no_tasks`，en/zh 对称）；`WorkspacePanel`/`TeamDeliverablesView`/`TeamTasksView` 三组件补 `use_i18n()`（文件已 import 宏）。
- **打磨话术**：「‘流式回显气泡’在 `views/chat/messages.rs`；‘右侧工作区时间线’在 `components/workspace_panel.rs`；‘工具卡片展开/折叠’共享态在 `state/layout.rs` 的 `expanded_events`（覆盖集，`toggle_event`/`is_event_toggled`），聊天侧与工作区侧同源同步、跨流式 remount 存活——别再往 `ToolCard` 塞卡内本地展开信号。这是前端 Leptos/WASM，改完要重编 binary（rust_embed，见 CLAUDE.md Panel↔Daemon 嵌入链）。」

### 6.2 Panel 远程连接与 Gateway Token 授权 (Remote Panel Connect & Gateway-Token Auth) ✅
- **口语关键词**：panel 远程连接、局域网连 core、Gateway token、token 授权框、二维码授权、QR 配对、壳连远程 core、授权后权限同本地、网页式登录
- **代码锚点**：
  - 传输/握手：`src/gateway/server/handler.rs`（WS `/ws` 升级；connect 握手在 `handler.rs:707-781` 取 `params.token` → 调 `connect_authorized` → 回填 `role`/`authorized`/`needs_token` 到 connect 响应 + stamp `caller_role`/`permissions` 到连接态）、`src/gateway/handlers/connect.rs`（`connect_authorized` 纯函数：loopback 恒 operator，远程须有效 token）
  - 登录墙：`src/gateway/server/handler.rs:512-543`（`caller_role != "operator"` 时除 `connect` 外全部拒（`AUTH_REQUIRED`））
  - Token：`src/gateway/security/shared_token.rs`（`SharedTokenManager`：`generate_token` / `validate` / `try_load_token_from_db` / `reset_token` 轮换，token 形如 `aleph-<uuid>`）、`src/gateway/security/store/tokens.rs`（`validate_shared_token_hash`）、boot 生成于 `commands/start/builder/subsystems.rs`
  - CLI：`src/bin/aleph-server/commands/bootstrap_token.rs`（`aleph-server bootstrap-token` 直读 `~/.aleph/data/security.db` 打印 token，供壳/QR 生成）
  - 前端：`interfaces/webchat/src/context.rs`（device_id 生成 + `handshake` 送 token + `capture_role` 捕获 `role`/`needs_token` + `read_gateway_token`(URL `?token=` > localStorage) + `scrub_token_from_url`(授权后清地址栏 token)）、`interfaces/webchat/src/components/token_wall.rs`（全屏登录墙 token 框）、`interfaces/webchat/src/views/settings/security/gateway_token.rs`（QR + LAN URL + rotate）
- **职责（单一真相）**：Panel 远程 = **纯 HTTP/WS 直连，不走 channel 通道**。壳 App 远程连 LAN core 的行为逻辑与「浏览器打开 core IP 网址」**完全一致**——同一条 Gateway-token 授权路径，壳无任何特权捷径。
  - **本机 (loopback)**：自动授权 = operator，**零配置**（壳内置 server / 浏览器 `127.0.0.1`）。信任边界 = 同机。
  - **远程 (LAN)**：首连未授权 → 弹 **Gateway token 输入框**（`token_wall.rs`），或扫 core 展示的 **二维码**（编码 `http(s)://<ip>:<port>/?token=<gateway-token>`，扫码即带 token 打开）→ `SharedTokenManager::validate` 通过 → 授权成功，**权限与本地完全一致（单层，无 Chat/Config 之分）**。客户端把 token 持久化（`localStorage`）供后续自动重连，并**清除地址栏 `?token=`**（不留痕、防过期链接锁死）。
  - **撤销**：轮换 Gateway token（旧 token 全部失效，所有远程端须重新输入）。无 per-device 会话（YAGNI）。
  - 前置：`[gateway] host = "0.0.0.0"` 才开放局域网到达；token 授权是到达之后的第二道闸（**在执行之前**，比旧模型「Chat tier 可未授权跑 bash」更强）。
- **状态**：✅ **已实现并对齐目标模型（2026-06-17）**。端到端连通：connect 校验 token（`handler.rs:707-781`）+ 登录墙（`handler.rs:512-543`）+ 单层收敛（`method_authz.rs` 仅余 channel tier 闸，Panel 无 Chat/Config 子层）+ 前端 token 框/QR + `bootstrap-token` CLI。`ChannelPermissionLevel` 仍为 channel 系统共用（未删，panel 已解耦）。**i18n + 泄漏修复（2026-06-19）**：① `token_wall.rs`（登录墙首屏）+ `settings/security/gateway_token.rs`（token 区）此前英文硬编码，已接入 i18n（`common.token_wall_*`/`gateway_token_*`，en/zh 对称）；② **安全**——`context.rs` WS 消息循环此前 `console::log` **整包 dump** 入站消息/事件（`"Received message: {value:?}"` 会把 `gateway.token.current` 响应里的明文 Gateway token + 全部对话内容打到浏览器控制台），已脱敏为 topic/id 级面包屑；③ `method_authz.rs` 回归测试补测 `agent_switch`（OPERATOR_TOOLS 已列但漏测）。
- **打磨话术**：「Panel 远程不是 channel，是**网页式 token 登录**且已落地。本机零配置 operator；远程输 Gateway token（或扫 QR）后**权限同本地，单层**。改‘授权判定’去 `connect_authorized`（纯函数，主机可测）；改‘登录墙’去 `handler.rs:512-543`；改‘前端 token 框/QR/持久化’去 `context.rs` + `token_wall.rs` + `settings/security/gateway_token.rs`。注意 `read_gateway_token` 让 URL `?token=` 优先于 localStorage——授权后必须 `scrub_token_from_url` 清掉，否则过期 QR 链接会锁死登录墙。改 Panel 记得重编 binary（rust_embed 嵌入链）。」

---

## 7. Desktop（桌面端）

> 桌面端是 **R1「大脑-四肢绝对分离」** 的物理落地：`src` 内只持有**能力契约 (Trait)**，真实平台 API（AppKit / Vision / windows-rs …）全在 `desktop/*` 原生 Bridge 子 crate，二者经 **JSON-RPC IPC** 跨界。本章按"契约 → 四肢 → 工具 → 壳 → 连线"组织。改桌面功能前先认清：**`src` 严禁直接调平台 API**（违 R1 不得合入）。
>
> **审计硬化（2026-06-19，§7.1-7.6 深度审计，28 子代理对抗验证）**：bridge stdin 写失败只 `fail(id)` 单请求、不再 `fail_all` 误伤共享 inflight 全表（7.1-a）；Wayland `delete` 修为 KEY_DELETE(111)、Linux 窗口 id 改变宽 hex 防 64 位 XID 截断（7.1-c/h）；`restart_app` 复用 `SystemCapability::restart_app` 去重 500ms 魔数（7.1-f）；set_of_marks 在 `scale_factor=None` 时回退 `display_list` 主屏 DPR，修 macOS Retina 标记错位 + 输出虚假 scale（7.3-a）；`focus_window`/`screen_record` 经新增 `requires_lock` 谓词取会话锁（与审批解耦、免提示，7.3-b）；scroll 小数 delta `round` + 零值守卫，杜绝静默 no-op 报成功（7.3-c）；`system` 工具补审批闸（launch/quit/restart→DesktopLaunchApp、clipboard_write→DesktopType），杜绝绕过 `desktop` 工具的闸（7.4-a）；permission 工具补全 14 种 PermissionKind（TCC 6 + 手动授权 8）文档（7.4-b）；删 PimArgs 6 个零消费字段（completed + 5 contacts 写字段）、`reminders_complete` 描述去伪、`mail_search` limit `clamp(1,200)`（7.4-c/d/f）；Gateway token 文件 cfg(unix) `0600`（7.5-a）；perm_monitor 改用 `menu::reload_panel` 硬 reload（7.5-c）；deeplink 改 `serde_json::to_string` 转义防换行破字面量（7.5-i）；macOS `screenshot{describe}` OCR 文本层经注入平台 `screen()`（bridge-backed）修复（此前用裸 NativeScreen，macOS OCR NotImplemented→静默丢失，7.6-a）；删 registry 死字段 `desktop_platform`（7.6-d）。**建议未做**：bridge 协议版本握手生产连线（7.1-b）——需注入最关键的 helper 自动 spawn 路径、与 `DEFAULT_RPC_TIMEOUT` 交互复杂、无法编译验证，且 core/helper 同包发版使实际偏移仅在异常手动换 helper 时发生，收益比低故推迟。

### 7.1 桌面能力契约与 Bridge IPC (Desktop Capability Contracts & Bridge IPC)
- **口语关键词**：大脑四肢分离、能力 trait、DesktopPlatform、Swift helper、JSON-RPC 桥、IPC、supervisor、能力契约
- **代码锚点**：`desktop/shared/`（`aleph-desktop` crate）——`traits/`（`screen.rs`/`system.rs`/`automation.rs`/`permission.rs`/`media.rs`/`pim.rs` + macOS 专属 `ax`/`power` 共八大能力 trait）、`platform.rs`（`DesktopPlatform` 聚合器，每能力返 `Option<&dyn XCapability>`，平台缺失即 `None`）、`bridge/`（`client.rs` + `codec.rs` + `supervisor.rs`(Backoff/RestartWindow/**SpawnGate**) + `inflight.rs`，stdio 上的 JSON-RPC 2.0 连 Swift helper 子进程）、`coord.rs` + `*_types.rs`（共享数据类型）；协议详解见 `docs/reference/DESKTOP_BRIDGE.md`
- **职责**：定义"桌面能做什么"的契约层（trait + 类型 + IPC 协议），不含任何平台实现。`DesktopPlatform` 是核心拿到的唯一抽象入口。
- **状态**：✅ 已实现（八能力 trait + DesktopPlatform 聚合 + JSON-RPC supervisor/backoff/inflight）。**Bridge supervisor 硬化（2026-06-17）**：① **修退避失效死 bug**——`ensure_running` 此前算出 `Backoff::next_delay()` 后 `let _delay=` **丢弃**（顶部 doc 谎称"backoff 已强制"），崩溃/spawn 失败以零间隔重试→毫秒内打穿 RestartWindow(5次/10min)→永久 `disabled`；现把散落的 `Backoff`+`RestartWindow`+"下次可 spawn 时刻"收口为单一内聚 `supervisor::SpawnGate`（poll/record_failure/record_success），退避真正强制（1s→2s→…→30s），窗口内返回新错误 `DesktopError::BridgeBackoff`（瞬时，区别于永久 `BridgeDisabled`）**不**重生；reader 崩溃路径也记退避。② **修双 spawn 竞态**——`ensure_running` 改为**持 `state` 锁跨越 check→spawn→store**（`spawn_process` 改为返回 `BridgeProcess` 不自锁），杜绝两个并发首呼各 spawn 一个 helper 留下孤儿 reader task 向窗口记幻影崩溃。③ **handshake 协议协商**——`BRIDGE_PROTOCOL_VERSION=2` 常量替魔数 + 校验 helper 返回版本不符即 `BridgeFailed`。④ **熵减**——删 `lib.rs` 死结构体 `Capability`（零构造零消费）+ 修过期模块 doc（4→8 能力）。
- **打磨话术**：「能力契约都在 `desktop/shared/traits/`；新增一种桌面能力 = 先加 trait + 类型，再各平台实现，**严禁在 `src` 直接调平台 API（违 R1）**。IPC 传输/方法表/错误信封在 `bridge/` + DESKTOP_BRIDGE.md。**helper 重生节流/崩溃恢复**全在 `bridge/supervisor.rs::SpawnGate`（单一真源，client 不再自管退避时刻）；‘helper 反复崩溃后多久才放弃’= RestartWindow 阈值，‘崩溃后多久才重试’= Backoff 阶梯 + SpawnGate 闸门（返回 `BridgeBackoff` 而非死等）。」

### 7.2 原生 Bridge 实现（四肢）(Native Bridge Implementations)
- **口语关键词**：macOS/Windows/Linux 原生实现、Swift bridge、AppKit/Vision、平台 OCR、四肢、平台特定代码
- **代码锚点**：`desktop/macos/src/`（Rust 侧 `screen.rs`/`ax.rs`/`automation.rs`/`permission.rs`/`pim.rs`/`system/` + Swift helper `desktop/macos/bridge/Sources/AlephBridge`）、`desktop/windows/src/`、`desktop/linux/src/`；各 crate 暴露 `MacOSPlatform`/`WindowsPlatform`/`LinuxPlatform` 实现 `DesktopPlatform`；构造装配点见 §7.6
- **职责**：每个 OS 一个 crate，提供 §7.1 契约的真实实现（macOS 经 Swift helper 触达 AppKit/Vision）。能力缺失返 `None`/`NotImplemented`，绝不 panic（P7）。
- **状态**：✅ 已实现（三平台 + macOS Swift helper）。**PIM「缺位域」默认化（2026-06-17）**：`PimCapability` 的 Apple 专属四域（Notes/Calendar/Reminders/Contacts 共 21 方法）已改为 trait 内 `default → NotImplemented`（对齐 `MediaCapability` 既有模式），单一真相源在 `desktop/shared/src/traits/pim.rs`；Windows/Linux 仅实现各自的 `mail_*`，删去各 ~16 个重复 stub；macOS 全量 override 不变。
- **打磨话术**：「平台真实实现按 OS 分 crate（macos 含 Swift helper）。改‘某平台某能力的实现’去对应 `desktop/<os>/src/<capability>.rs`；这是‘四肢’，绝不在 `src`（大脑）里写平台代码。**某能力在某平台天然缺位**？别在该平台 crate 写 stub——把方法在 §7.1 的 trait 里给 `default → NotImplemented`，缺位平台自动继承（PIM 四域已如此）。」

### 7.3 桌面控制与 GUI 工具 (Desktop Control & GUI Tools)
- **口语关键词**：screenshot、点击、GUI 自动化、set-of-marks、accessibility 树、视觉定位、屏幕操作、操控桌面
- **代码锚点**：`src/builtin_tools/desktop/`——`mod.rs`（DesktopTool 入口：安全硬阻断/审批/会话锁/escape 中止/coord 归一化/batch 展开）、`native.rs`（平台分发大 match：screenshot/ocr/click/double_click/drag/scroll/launch_app/quit_app/window_list/focus_window/screen_record…，**screenshot 与 screen_record 共用 `screen_region_from_args` region 校验**）、`types.rs`（DesktopArgs/DesktopOutput）、`ax.rs`（无障碍树查询 4 工具）、`set_of_marks.rs`、`gui_locate.rs`、`browser_operator.rs`、`vision_bridge.rs`（OCR 文本层，喂纯文本模型）、`coord_resolve.rs`（含 region 归一化重缩放）、`safety.rs`、`session_lock.rs`
- **职责**：LLM 面向的桌面控制工具集，全部经 `DesktopPlatform` 调四肢；视觉定位（set-of-marks / gui_locate / ax）把屏幕变成可点击的结构化标的。
- **状态**：✅ 已实现，含坐标解析 + 安全护栏 + 会话锁。
- **打磨话术**：「LLM 控制桌面的工具都在 `builtin_tools/desktop/`；‘点哪/看哪’的视觉定位走 `set_of_marks` + `gui_locate` + `ax`。工具只调 `DesktopPlatform`，不碰平台 API。」

### 7.4 系统类桌面工具 (System / Automation / Permission / Media / PIM Tools)
- **口语关键词**：通知、剪贴板、启动应用、系统信息、AppleScript/快捷指令、相机/录音/语音转写、备忘录 PIM、权限检查/请求
- **代码锚点**：`src/builtin_tools/system_tool.rs`（启动/退出/**重启**应用、通知、剪贴板、系统信息、idle）、`automation_tool.rs`（脚本/快捷指令，**已接 approval 闸**）、`permission_tool.rs`（权限检查/请求/**引导/开设置**）、`media_tool.rs`（相机/录音/STT/mic）、`pim/`（备忘录/日历/提醒/联系人 + **Mail 检索**）；均经对应 `DesktopPlatform` 能力
- **职责**：DesktopTool 之外的系统能力工具——一能力一工具，全经 `DesktopPlatform` 路由到四肢。
- **状态**：✅ 已实现。**连线补全（2026-06-17）**：此前若干 trait 能力已全平台实现却无工具 action 暴露（悬空四肢），现已逐项连线：① **PIM Mail**——`PimCapability::{mail_search,mail_get,mail_folders}`（macos/windows/linux 三平台全实现）此前 `pim` 工具零 mail action，现接入（只读免审批，新增 `limit` 参数）；② **Permission 引导**——`permission` 工具补 `guide`（深链+步骤+理由 `guide_permission`）+ `open_settings`（开系统设置面板），文档承诺的"引导"此前缺失；③ **System restart_app**——暴露 `SystemCapability::restart_app` 默认方法；④ **错误修复/熵减**——`pim` 写操作审批此前借 `ActionType::DesktopClick` 占位（代码内 TODO，策略无法区分桌面点击 vs PIM 写），现新增专属 `ActionType::PimWrite`；⑤ **安全增强**——`automation.run_script`（任意 AppleScript/JXA/shell/PowerShell 代码执行）+ `run_shortcut` 此前**完全无审批**（而 PIM 日历写却受闸，安全不一致），现接入 approval policy 经新 `ActionType::DesktopAutomation`（permissive 默认 Allow 保持 byte-identical，用户可在 `~/.aleph/approval-policy.json` 设 `desktop_automation: ask/deny` 收紧）。两个新 ActionType 已同步进 `config.rs` 的 permissive(Allow)/strict(Ask) 两张默认表。
- **打磨话术**：「这些是 GUI 控制之外的系统能力工具，一能力一工具。加新系统能力先回 §7.1 看契约有没有对应 trait，没有先加 trait。**注意契约方法 ≠ 工具 action**：trait 实现了不等于 LLM 够得到，要在对应 `*_tool.rs` 的 action match 里连出来（参 2026-06-17 Mail/guide/restart_app 连线）。‘脚本/快捷指令要不要审批’= `automation_tool.rs` 的 `DesktopAutomation` 闸；‘PIM 写审批’= `PimWrite`（不再借 DesktopClick）。」

### 7.5 Tauri 桌面壳 (Desktop Shell)
- **口语关键词**：桌面 App、Tauri、托盘、系统通知、daemon 生命周期、auto-update、`aleph://` deeplink、全局唤起热键、应用菜单、连远程 core
- **代码锚点**：`desktop/shell/`（`aleph-desktop-shell` crate）——`src/`（`main.rs`/`daemon.rs`(启动+监督 detached daemon)/`tray.rs`/`notify.rs`(EventBus→原生通知桥；远端 `connect` 经 `connection::load_gateway_token` 附带 Gateway token，握手被拒即结束 session 退避重连)/`menu.rs`/`hotkey.rs`(全局唤起)/`update.rs`(后台自更新)/`deeplink.rs`+`external_link.rs`(`aleph://`)/`perm_monitor.rs`/`webview_perms.rs`/`connection.rs`+`connect_setup.rs`(连本地或远程 Gateway，target 持久化 `~/.aleph/.desktop-shell-target`，远端 token 从 `?token=` 提取持久化 `~/.aleph/.desktop-shell-gateway-token`)）、`tauri*.conf.json`、`build.rs`、`Info.plist`/`Entitlements.plist`；文档 `docs/reference/DESKTOP_SHELL.md`
- **职责**："最后一公里"原生外壳：把 Panel（Leptos/WASM）装进原生窗口，提供托盘/通知/自启/自更新/deeplink/热键/菜单 + daemon 生命周期。**零业务 UI、零业务逻辑**（R2/R10）——UI 在 Panel、推理在 daemon。
- **状态**：✅ 已实现（Tauri v2）。远端通知桥 token 连线已补（2026-06-17）：`set_connection_target` 从 QR/分享链接 URL 的 `?token=` 提取 token 存盘，`notify.rs` 远端 `connect` 携带之 → 远端 token 网关也能弹原生 R5 通知；并修复此前裸连被 `AUTH_REQUIRED` 拒（id 2）后静默死锁、永不重连的缺陷（`handshake_error` 结束 session→指数退避重连）。capability 仅放行 loopback，故走壳侧 URL 提取而非远端 Panel→壳 IPC。
- **打磨话术**：「壳是纯 I/O + OS 集成，**别往里加业务 UI/逻辑（违 R2/R10）**。改窗口/托盘/更新/热键在 `desktop/shell/src/`；‘连本地还是远程 core’在 `connection.rs`。**远端原生通知**靠 `connection.rs` 从远端 URL 的 `?token=` 提取并持久化、喂给 `notify.rs` 的 `connect`（Local/loopback 不带 token）；手动输地址+登录墙输 token 的路径壳侧拿不到 token，是已知限制（桥优雅退避而非死锁）。注意 Panel 经 rust_embed 编译期嵌入 daemon，改 Panel 看不到效果是漏了重编 binary（见 CLAUDE.md 嵌入链）。」

### 7.6 核心侧连线与 daemon 消费者 (Core Wiring & Daemon Consumers)
- **口语关键词**：桌面能力注入、per-OS 构造、单一注入点、power inhibit、防休眠、presence、麦克风电平、平台 OCR
- **代码锚点**：构造/装配 `src/executor/builtin_registry/builder/constructor.rs`（按 OS `new` 对应 `DesktopPlatform` + 装配全部桌面工具 + VisionBridge）、`src/bin/aleph-server/commands/start/orchestrator_init.rs`（`power` 能力注入）、`src/harness/deps.rs`（`power` 字段——turn 进行中抑制系统休眠）；**daemon 侧消费者（非工具）** `src/tasks/presence/`（周期广播 system_info + user_idle）、`src/tasks/mic_level/`、`src/vision/providers/platform_ocr.rs`（`ScreenCapability` → OCR 视觉 provider）
- **职责**：桌面能力的**唯一注入点**在 `constructor.rs`（per-OS 选择 Platform，依赖倒置 P4）；power 用于 turn 内防休眠；presence/mic/OCR 是 daemon 后台对桌面能力的消费者。
- **配置连线**：presence/mic 的策略（开关 / 周期 / 阈值）经 **`[desktop]` 段**驱动（`src/config/types/desktop.rs` 的 `DesktopDaemonConfig` 复用 `tasks::{presence,mic_level}` 的 config 结构，零重复 schema）；boot 路径 `start/mod.rs` 读 `config.desktop` 后**只构造一次平台 Arc** 共享给两个 reporter（段缺省＝presence 开@30s / mic 关，与历史硬编码行为字节一致）。
- **状态**：✅ 已实现（含 `[desktop]` 配置连线——此前 reporter 跑 `::default()`，mic_level 整特性无配置路径可开启）。
- **打磨话术**：「桌面能力的**单一注入点**在 `constructor.rs`（按 OS new Platform）；‘turn 中防休眠’在 `deps.rs` 的 power；presence/mic/平台 OCR 是 daemon 侧**消费者**不是工具——找‘桌面能力被谁用了’来这里。调 presence/mic 的开关与周期去 `[desktop.presence]` / `[desktop.mic_level]`。」

---

## 附录 A. 实现现状体检（⚠️/❌ 清单——打磨时最该先看）

| # | 功能 | 状态 | 现状 vs 直觉的差距 | 若要"做成描述的样子"的性质 |
|---|------|------|---------------------|----------------------------|
| 1 | doctor+f LLM 修复 | ✅ G1 已实现 2026-06-16 | Panel `f` 入口已加（带编辑焦点护栏）；「LLM 修复」= 注入 prompt 走现有 loop+工具（doctor 后端零改动，结构化 findings 早已喂 LLM） | ~~新功能~~ 已完成 |
| 2 | Panel 远程 token 授权（单层） | ✅ **已实现 2026-06-17** | 已收敛单层：connect 校验 token + 登录墙（`handler.rs:512-543`/`707-781`）；两层 device tier 已退场，`method_authz.rs` 仅余 channel tier 闸 | **完成**，见 §6.2 |
| 3 | Gateway token 输入框 / QR 授权 | ✅ **已实现 2026-06-17** | token 框 `token_wall.rs` + QR/LAN URL/rotate `settings/security/gateway_token.rs` + `bootstrap-token` CLI 已复活；授权后 `scrub_token_from_url` 清地址栏 token（修复过期 QR 链接锁死登录墙） | **完成**，见 §6.2 |
| 4 | ~~kimi vs claude 差异化压缩阈值~~ | ✅ **G4 已实现 2026-06-16** | 窗口比例自动浮动 **+** `[[context_budget.model_thresholds]]` per-model 阈值覆盖（matcher=model id/provider key 子串，逐项回退全局，过防御闸） | **新增配置完成**，见 §2.2 |
| 5 | DAG 工具执行 | ✅ G5 已澄清 2026-06-16 | 工具层=资源群分并行（`concurrency.rs`，非真 DAG）；真任务 DAG 在 `workflow/compile.rs`+`teams/dispatcher/` | ~~描述时分清~~ 已在 §3.3 / §4.3 / §4.4 / 术语表四处区分；**仅澄清，无需开发** |
| 6 | ~~错误沉淀教训(三支柱③)~~ | ✅ **G6 已查证 2026-06-16** | 端到端已连且存活（flag_user_correction→FeedbackDistill→feedback note→召回）；auto error-hook 故意不做(R7/R10) | **零代码完成**，见 §2.5③ |

## 附录 B. 高频"混称"对照（说清楚指哪个）

- **"语音"**：context 注入侧（`thinker/layers/voice_mode.rs`）≠ 运行时 ASR/TTS（`gateway/voice/`）。
- **"DAG"**：工具并发群分（`tools/concurrency.rs`）≠ 任务依赖图（`workflow/compile.rs` + `teams/dispatcher/`）。
- **"权限"**：approval 三级引擎（`src/approval/`）≠ Panel 的 Gateway-token 授权（授权后单层＝同本地，§6.2）≠ sandbox 命令策略（`src/sandbox/command_policy/`）。
- **"hook"**：stop hook ≠ sandbox hook ≠ extension shell-hook consent（§5.10 三套）。
- **"命令/工具/斜杠命令"**：同一套 ToolCatalog（§3.5），不是三套。
- **"插件"**：plugins ⊂ extension（`src/extension/`），不是独立目录。
- **"loop vs cron"**：loop 内存态随会话消亡（`src/looping/`）；要持久周期任务用 cron（`src/tasks/cron/`）。
- **"desktop"**：能力契约 trait（`desktop/shared`，大脑只持有它）≠ 原生 Bridge 实现（`desktop/{macos,windows,linux}`，四肢）≠ LLM 桌面工具（`src/builtin_tools/desktop` + system/automation/permission/media/pim）≠ Tauri 桌面壳（`desktop/shell`，纯外壳）。说"桌面"时指明是契约 / 四肢 / 工具 / 壳哪一层（§7）。
</content>
</invoke>
