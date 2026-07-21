# MODE_SYSTEM.md — 会话使用模式 Chat / Work / Code (Session Mode System)

> 2026-07-21 落地。Spec 与调研母本见
> [docs/superpowers/specs/2026-07-21-chat-work-code-mode-system.md](../superpowers/specs/2026-07-21-chat-work-code-mode-system.md)。

## 1. 是什么 (What)

一根**用户显式选择**的会话级旋钮 `SessionMode { Chat, Work, Code }`（默认 `Work`），
与 `ExecTier`（自治/审批）、`ThinkLevel`（推理深度）**完全正交**的第三孪生。
模式**不授予、不拒绝任何权限**——安全永远归 exec tier × `tool_permissions` × sandbox；
模式改变的是工具**呈现面的资源分配**与 prompt 语域，让 agent 在不同工作形态下更高效：

| | chat 聊天 | work 工作（默认） | code 编码 |
|---|---|---|---|
| 定位 | 轻量对话 / 问答 / 记忆 | 多步骤知识工作、交付物 | 软件开发 |
| schema 常驻核 | 默认核 − dev 工具（bash/code_exec/code_check/file_write/file_edit/file_ops 折叠为占位，`get_tool_schema` 按需取回） | `[tools] core` 原样（**字节等价旧行为**） | 默认核 + `apply_patch`/`ctx_search` |
| 整族延迟（deferred，`tool_search` 可发现+晋升） | desktop/browser/team/task/node/arena/media/生成族/cron/automation/heartbeat/goal/loop/workflow/google_meet/hub/clawhub/skill_install·manage/a2a/acp/gateway_route/apply_patch/code_check/strategy/vault_store | 无 | desktop/media/生成族/google_meet |
| prompt | `Usage mode: chat …` 一行 | `Usage mode: work …` 一行 | `Usage mode: code …` 一行 |
| Panel 右栏 | 不自动驱动（badge 计数） | 有计划后 live-follow Plan/进度面 | live-follow 工具/diff（现状） |

永不延迟的生命线：`tool_search` / `get_tool_schema` / `ask_user` / `session_set_mode` / `self_config`
（`NEVER_DEFER`——延迟发现机制本身会把模型困死在分区里）。

## 2. 红线定位 (Redline Position)

- **R10 渐进披露例外的标准形状**：不看消息内容的静态分区（用户选择 + 工具名/族前缀），
  加载/晋升决策 100% 由模型经 `tool_search`/`get_tool_schema` 发起；落在工具呈现层
  （`DeferredTools` + `ProgressiveDisclosureRewriter` 两套**既有机制的输入端**），
  `src/harness/` **零行**（budget.rs 棘轮验证通过）。
- **禁止**：任何按消息内容推断/切换模式的代码（R7/R10-#2）。模式只来自：用户 pill、
  `[policies] mode` 全局默认、或模型的 `session_set_mode` 工具调用（用户要求时）。
- **调研收敛**（Codex/Claude/Kimi/Cursor/Zed/Copilot/Continue/Roo 等 20+ 产品）：
  模式的首要杠杆是工具可用面；能力模式与自治度是两根不可混淆的正交旋钮；composer
  是模式切换器；右栏是模式的信号面。

## 3. 代码锚点 (Anchors)

**单一源**：`src/config/types/policies/session_mode.rs` —— `SessionMode` + `MODE_SESSION_KEY`
(`"session_mode"`) + `from_id/id` + `builtin_modes()`（只发 id，copy 归 Panel，R4/R6）+
`prompt_line()`（copy 与分区表同文件，R9）+ 分区表
（`CHAT_CORE_SUBTRACT`/`CODE_CORE_ADD`/`*_DEFER_PREFIXES`/`NEVER_DEFER`）。

**三孪生管道**（与 exec_tier / think_level 逐点同构）：

1. 载体入口：`chat.send`/`agent.run` 顶层 `mode` 参数（`SendParams.mode` → `AgentRunParams.mode`，
   `handlers/chat.rs` + `handlers/agent.rs`，双入口 `server_init.rs` 同步）；
2. 校验+stamp：`build_run_request`（`handlers/agent.rs`，未知 id fail-loud）→
   `RunRequest.metadata[MODE_SESSION_KEY]`；
3. 每轮解析：`src/gateway/execution_engine/turn_mode.rs::{resolve_turn_mode,resolve_session_mode}`
   （request > session > global，stamp-on-carry best-effort，malformed fail-soft；无 channel clamp——
   模式非安全边界）；
4. 会话持久：`identity_meta.custom["session_mode"]`；`sessions.patch` 配对校验
   （`db_handlers/modify.rs`，null=清除跟随全局）+ 列表投影 `SessionInfo.mode`
   （`db_handlers/{query,types}.rs`）。

**分区落点**（`run_loop/inner.rs`）：`mode_core_tools = session_mode.effective_core_tools(...)`
喂进两处 `build_request_tool_service` 与 SchemaLookupTool 注册门；mode 延迟名并入
`DeferredTools` 构造集（与 `defer_mcp_tools` 同一 Arc，`ToolSearchTool` 晋升路径不变）。

**prompt line**：`FlowRequest.session_mode` → `HarnessRunner::run` 参数 →
`resolve_prompt_context` → `ResolvedContext.session_mode` → `SecurityLayer`
（`Usage mode:` 行，紧邻 `Approval mode:` 行；会话稳定，仅用户翻转时 re-key）。

**R8 工具**：`src/builtin_tools/sessions/set_mode_tool.rs`（`session_set_mode`，
镜像 `session_rename` 的 8 个注册点：definitions/groups/struct_def/constructor/
optional_tools/tool_registry_impl dispatch/sessions mod/别名无）。

**全局默认**：`PoliciesConfig.mode`（`[policies] mode = "chat|work|code"`），
`config.get_tool_permissions` 响应携带 `mode` + `modes`（与 tier 同一 fetch 同一解码器），
`config.update_tool_permissions` 接受 `mode` 部分更新。

**Panel**（`interfaces/webchat/`）：`views/chat/mode_picker.rs`（composer pill，
exec_tier_picker 克隆）+ `components/mode_labels.rs`（i18n 显式 match，未知 id 降级裸 id）+
`ChatState.session_mode`（5 生命周期位点 + SessionSnapshot）+ 6 条 `ChatApi::send`
调用路径全带 + `chat_sidebar.rs` 重选还原 + locales `settings.policies.mode_*`（en/zh）。
右栏：`state/layout.rs::follow_plan`（work 模式 live-follow 变体，同 pin 契约）+
`views/chat/events.rs` 的 `tool_call_started` 前台分支按模式分派（chat=不驱动 /
work=有计划后 follow Plan / code 与跟随全局=follow 工具）。

## 4. 刻意不做 (Deliberately NOT)

1. 不绑 per-mode 模型（`select_model`/agent hint 已覆盖；session_model_handle 重启即失）；
2. 不动 PromptMode Full/Compact/Minimal（prompt_mode.rs 实测警告）；
3. mode 不携带/不联动 exec tier（两 pill 各自显示、互不写入）；
4. 不做 mode 的 channel clamp（模式非安全边界）；
5. 不做自定义模式（三内建；`session_mode.rs` 的声明式表为未来留形，零现实消费者不预建）；
6. 不做 per-mode compaction/context budget（无 per-run 缝）；
7. 不绑 `FlowOverrides.extra_system_prompt`/`context_mode`（零消费者死字段，CUT 候选）；
8. 右栏不建新 surface/tab 系统（只做模式条件默认行为，在 InspectorTarget 缝内）。

已知限制（v1）：Panel 右栏行为只认**会话显式 override**——跟随全局的会话走默认
follow 行为（全局 mode 未 plumb 进事件处理层；如需，后续把全局默认挂进共享 context）。

## 5. 排查话术 (Troubleshooting)

- 「为什么这个工具不在列表里」→ 先看会话模式：chat/code 的整族延迟是**软分区**，
  模型 `tool_search` 一次即晋升；exec tier 的 Deny 才是硬隐藏。
- 「模式切了没生效」→ 分区在**下一轮**重建（工具面 per-request 构建）；查
  `resolve_turn_mode` 的三级来源与 `identity_meta.custom["session_mode"]` 实值。
- 新增工具族时：想让 chat/code 模式收起它，改 `session_mode.rs` 的前缀表**一处**即可；
  绝不在别处再建第二张表。
