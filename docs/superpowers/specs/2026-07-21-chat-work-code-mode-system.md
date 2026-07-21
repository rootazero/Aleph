# Spec: Chat / Work / Code 三大使用模式 (Session Mode System)

日期：2026-07-21
状态：已实施（同日）
产物：本 spec + plan（`docs/superpowers/plans/2026-07-21-chat-work-code-mode-system.md`）+ `docs/reference/MODE_SYSTEM.md`

---

## 1. 目标 (Goal)

为 Aleph 构建用户可选的三大使用模式 **chat / work / code（聊天 / 工作 / 编码）**：

- 不同模式呈现不同的工具面与 prompt 语境，让 agent 运行更高效（更少常驻 schema、更聚焦的行为语域）；
- 在 Panel chat 窗口 composer 添加模式选择器（对齐 exec-tier pill 模式）；
- 不同模式下右侧栏 (WorkspacePanel) 差异化利用：chat 不打扰、work 面向交付物进度、code 面向工具/diff 跟随。

## 2. 调研结论（10 路输入的收敛信号）

调研面：OpenAI Codex（CLI/IDE/App/ChatGPT 桌面 Chat|Work|Codex 三分）、Anthropic Claude（Desktop Chat|Cowork|Code 三 tab + 6 权限模式）、Kimi（OK Computer / 模式收敛进模型选择器）、Cursor/Windsurf/Copilot/Zed（Ask/Edit/Agent 三分法）、跨消费产品扫描（ChatGPT/Manus/Warp/Perplexity/Gemini/Grok/Raycast）、GitHub 开源 12+ 项目（Roo-Code/Kilo/Cline/Continue/VS Code/Zed/void/goose/Aider/OpenHands/Cherry Studio 等）。

跨产品一致的设计原理：

1. **模式 = 用户显式选择的静态预设**，首要杠杆是**工具可用面**（Zed：profile 就是工具表；Copilot custom agent 声明 `tools:`；Continue：mode 整表换 permission policies），prompt 片段其次，模型/effort 第三。
2. **能力模式与自治度是两根正交旋钮，绝不混淆**（Codex：sandbox × approval 双轴，用户模式只是矩阵上的命名点；Zed：profiles 决定"有没有"，Tool Permissions 决定"要不要问"）。→ Aleph 的 mode ⊥ exec tier ⊥ think level。
3. **composer 就是模式切换器**——所有产品的模式选择都在输入框内/旁（pill/dropdown/chips），无一用顶层导航切模式。
4. **右侧栏就是模式的信号**：纯 chat 无面板；work 模式召唤 artifacts/进度/任务面板（Claude Cowork 三栏：任务列表/对话/Progress+Artifacts+Context）；code 模式召唤 diff/文件/终端面板（Codex review pane、Claude Code 可拖拽 pane 系统）。"审查面与工作产物同形"（action→diff、plan→文档、deliverable→预览）。
5. **按产出形态分模式，不按能力档位**（ChatGPT 2026-07 官方口径：Chat 管对话、Work 管交付物、Codex 管软件开发；三者历史隔离、权限面隔离）。
6. **Continue 三动词之辨**：`allow` / `ask` / `exclude`——exclude 是"不在 schema 里"，ask 是"在但要问"。模式管 exclude/资源分配，审批档管 ask。
7. **void 的单源分类器**：复用既有工具元数据（审批类型/只读注解）推导模式分区，不建第二张表。
8. **模式切换要模型可见、可驱动**（Roo `switch_mode` 常驻工具；Cline `<mode_notice>` 打进 transcript；Continue 的 chat prompt 明说"可建议用户切 Agent 模式"）。
9. **作用域：per-session，写进会话记录**（Continue session.mode；goose create_session(mode) 继承；Cline 全局模式反例——被迫加补偿机制防跨任务泄漏）。
10. **声明式记录，不是散落 match 的硬编码枚举**（VS Code 内建模式最终折进 custom-agent 机制；Zed 内建 profile 定义在 default.json 数据里）。

## 3. 设计决策 (Decisions)

### D1. Mode 是第三根正交旋钮（三孪生）

`SessionMode { Chat, Work, Code }`，默认 `Work`。与 `ExecTier`（自治/审批）、`ThinkLevel`（推理深度）完全正交，复用同一根会话管道：

- 单源常量 `MODE_SESSION_KEY = "session_mode"`（对齐 `EXEC_TIER_SESSION_KEY` / `THINK_LEVEL_SESSION_KEY`）；
- 载体：`chat.send` 顶层参数（首条消息问题）→ `build_run_request` 校验（未知 id fail-loud 拒绝）+ metadata stamp → `resolve_turn_mode`（`turn_permissions.rs`/`turn_thinking.rs` 的第三孪生）→ request > session > global 优先级 → stamp-on-carry 持久化到 `identity_meta.custom["session_mode"]`；
- 全局默认：`[policies] mode` (`PoliciesConfig.mode`)，Settings 可改，逐 turn live 读；
- `sessions.patch` 侧配对校验（modify.rs，null 合法=清除跟随全局）+ sessions 列表投影（query.rs → SessionInfo.mode）供 Panel 重载还原。

### D2. 工具分区语义 = 资源分配（resident vs collapsed vs deferred），不是 deny

安全（deny/ask）永远归 exec tier + tool_permissions + sandbox；mode 只调**呈现层**两套既有机制的输入：

- **core 集（schema 常驻）**：mode 化 `[tools] core` 的有效集合——本模式核心工具保留全量 schema，其余折叠为占位（`get_tool_schema` 按需加载）；
- **deferred 集（列表延迟）**：明显域外的工具族整族延迟——不进初始工具列表，`tool_search` 可发现并晋升（R7 主权：一切工具对模型永远可达）。

这正是 R10 渐进式工具披露例外的形状：**不看消息内容的静态分区 + 加载决策 100% 由模型发起**，落在 `src/tools/scoped/`（唯一强制点），`src/harness/` 零行增长。

### D3. 三模式语义与分区表（单源，定义在 Mode 枚举旁）

| | **chat 聊天** | **work 工作**（默认） | **code 编码** |
|---|---|---|---|
| 定位 | 轻量对话、问答、记忆 | 多步骤知识工作、交付物、通道/日程/媒体 | 软件开发全链路 |
| schema 常驻核 | 对话最小核（search/web_fetch/memory/note/ask_user/meta 工具） | 现状默认核（= `default_core_tools`，行为向后兼容） | 默认核 + 开发核权重（bash/code_exec/code_check/file_* 全量保障） |
| 整族延迟 | 桌面/媒体/团队/集群/goal/loop/cron 等重族 | 集群 node_* 等少量远域 | 无额外延迟 |
| prompt line | 一行语域声明（对话、克制、不主动起长任务） | 一行（交付物导向、计划可见） | 一行（工程语域、验证习惯） |
| 右侧栏 | 不自动打开（badge 计数即可） | run 开始自动开 Plan/Progress 面 | 现状 live-follow 工具/diff + 文件抽屉 |

分区一律按**工具名/族前缀**声明（静态、内容盲），表与 Mode 枚举同文件（R9：规则与描述同处，防漂移）。元工具豁免不变：`tool_search`/`get_tool_schema`/`subagent`/session 工具在任何模式常驻（沿用既有自插入豁免机制）。

### D4. Prompt：一行 `Mode::prompt_line()`，走 Cached 路径

对齐 `ExecTier::approval_prompt_line` 的安排：copy 与规则同文件；经 `ResolvedContext` → 既有 prompt layer 渲染；会话稳定、仅用户翻转模式时 re-key（缓存纪律，FEATURE_LOCATOR §1.1）；同时告知模型可用 `session_set_mode` 切换（模型可驱动，D6）。遵守 prune-the-prompt：一行运行时事实，不教模型思考。

### D5. Panel：模式选择器 pill + 右侧栏差异化

- **pill**：克隆 exec_tier_picker 全套载体模式——`ChatState.session_mode: RwSignal<Option<String>>`（5 个生命周期位点 + SessionSnapshot）、3 条发送路径（typed/flush_queue/voice）全带、sidebar 重选直接还原 signal（不经 select 防冗余 patch 写）、`mode_labels.rs` 显式 match i18n（en/zh，未知 id 降级裸 id）、核心经 RPC 下发 id 列表（core 管身份，Panel 管文案）；
- **右侧栏**：在既有 `WorkspacePanel + InspectorTarget` 缝内做**模式条件的默认行为**（不建新 pane、不违反 state/inspector.rs 的静态 variant 契约）：chat 抑制 live-follow 自动开面；work run 开始 `inspect(Plan)`；code 保持现状工具跟随。

### D6. 模型可见、可驱动：`session_set_mode` R8 工具

沿 `set_topic_tool.rs` 形状（TURN_CONTEXT 发现会话，写 `MODE_SESSION_KEY`），LLM 可对话式切模式（"帮我进入编码模式"）。切换下一 turn 生效（分区经 cache generation 失效重传工具面）。

## 4. 红线合规 (Redline Compliance)

- **R10 五不之二**：mode 是用户显式选择的静态分区，与 allowlist/permission/health/deferred 四道 retain 同层同性质，属渐进披露例外；任何按消息内容选模式的代码都是违规（spec 明令禁止）；
- **R10 12 文件/行数棘轮**：`src/harness/` 零改动——一切经 FlowRequest 既有字段与 `src/tools/scoped/` / `src/gateway/execution_engine/` 落地；
- **R7/R8/R9**：模式切换语义交给 LLM（工具+prompt line），代码只做静态分区；对话即管理（session_set_mode）；智慧在 prompt（一行语域声明）；
- **唯一强制点**：分区全部落 `src/tools/scoped/`；slash 快路径经共享 resolve 同源（mode 不影响权限判定，故快路径天然无旁路——分区只作用于呈现层 list/schema，不作用于 execute 合法性）；
- **R4/R6**：Panel 只挑 id + 渲染文案；解析/校验/持久化全在 gateway。

## 5. 刻意不做 (Deliberately NOT doing)

1. **不绑 per-mode 模型**——`select_model` R8 + agent hint 已有；session_model_handle 重启即失（内存态），绑定反而造成"模式静默降级"；未来若做，走 UI 建议不强制（Continue 警告图标模式）；
2. **不动 PromptMode Full/Compact/Minimal**——in-code 实测警告"不得代用户选择"（prompt_mode.rs:6-36）；
3. **mode 不携带/不修改 exec tier**——保持正交（评估过"mode 设默认 tier"，v1 拒绝：pill 各自显示，互不写入；避免用户在两旋钮间产生隐式联动预期）；
4. **不做 channel clamp for mode**——mode 非安全边界，chat-tier 通道请求任何 mode 无害（安全仍由 exec tier 的 channel clamp 兜底）；
5. **不做自定义模式**——YAGNI；三内建以声明式记录表达，schema 已为未来自定义留路（GitHub lesson 1/10 的收敛终点，但零现实消费者不预建）；
6. **不做 per-mode compaction/context budget**——无 per-run 缝，发明缝=把策略推向 loop（映射结论）；
7. **不绑 `FlowOverrides.extra_system_prompt` / `FlowOverrides.context_mode`**——零消费者死字段（映射发现，另列 CUT 候选，本轮不动）；
8. **右侧栏不做新 surface/tab 系统**——只做模式条件默认行为；Canvas/Browser stub 的毕业另行立项。

## 6. 验收 (Acceptance)

- `resolve_turn_mode` 优先级 request > session > global 有 pinned 测试；未知 id 在 chat.send 与 sessions.patch 两写路径同拒；
- 三模式的分区差异有 scoped 层测试（list/metadata_schema/describe/dispatchable_list 四镜像一致）；模式翻转后 cache generation 失效；
- 元工具在任何模式可达（豁免测试）；
- Panel pill 三发送路径带值、快照/重载还原、i18n 双语齐备；
- `cargo check --bin aleph-server` + 相关模块 `cargo test` 绿；wasm 构建通过。
