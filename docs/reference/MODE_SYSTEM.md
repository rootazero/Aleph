# MODE_SYSTEM.md — 会话使用模式 Chat / Work / Code (Session Mode System)

> 2026-07-21 落地（v1）；同日 v2 深化轮（Workflow 调研 4 路 + 审计 3 路 + 对抗验证，
> 19 项确认发现全部修复/落地）。Spec 与 v1 调研母本见
> [docs/superpowers/specs/2026-07-21-chat-work-code-mode-system.md](../superpowers/specs/2026-07-21-chat-work-code-mode-system.md)。

## 1. 是什么 (What)

一根**用户显式选择**的会话级旋钮 `SessionMode { Chat, Work, Code }`（默认 `Work`），
与 `ExecTier`（自治/审批）、`ThinkLevel`（推理深度）**完全正交**的第三孪生。
模式**不授予、不拒绝任何权限**——安全永远归 exec tier × `tool_permissions` × sandbox；
模式改变的是工具**呈现面的资源分配**与 prompt 语域，让 agent 在不同工作形态下更高效：

| | chat 聊天 | work 工作（默认） | code 编码 |
|---|---|---|---|
| 定位 | 轻量对话 / 问答 / 记忆 | 多步骤知识工作、交付物 | 软件开发 |
| schema 常驻核 | 默认核 − dev 工具（bash/code_exec/file_write/file_edit/file_ops 折叠为占位，`get_tool_schema` 按需取回；code_check 走整族延迟不进减表） | `[tools] core` 原样（**字节等价旧行为**） | 默认核 + `apply_patch`/`ctx_search` |
| 整族延迟（deferred，`tool_search` 可发现+晋升） | desktop/browser/team/task/node/arena/生成族/cron/automation/heartbeat/goal/loop/workflow/google_meet/hub/clawhub/skill_install·manage/a2a/acp/gateway_route/apply_patch/code_check/strategy/vault_store/session_collaborate·turn 族 + 精确名 media/media_send | 无 | desktop/google_meet/生成族 + 精确名 media/media_send |
| prompt | `Usage mode: chat …` 一行 | `Usage mode: work …` 一行（deliverable-first 语域） | `Usage mode: code …` 一行（技术细节欢迎语域） |
| Panel 右栏 | 三档相同：产物面板（交付物置顶 / 附件 / 工作区文件），不随模式变化 |||

> ⚠️ **右栏一行的订正（2026-07-26）**：上表此前写「chat 不自动驱动 / work live-follow Plan / code live-follow 工具·diff」，那描述的是**已删除的** `components/inspector/`。右栏现在是产物面板（FEATURE_LOCATOR §6.7），**不按模式分化**——模式只管工具呈现面，不管右栏渲染什么。
>
> **`artifact_publish` 三档常驻**（不在任何 defer 表里，`session_mode.rs` 的两处测试钉住）：把成品交到用户手里在 chat 里同样是合法结局（"帮我写成一份报告"），而它的 schema 只有一个小对象。

**匹配语义（v2）**：族条目在 `_` 词边界匹配（`matches_family`——`desktop` 命中 `desktop`/`desktop_som`，
永不命中 `desktops`；`goal` 不误伤 `goals_list`）；**MCP 限定名（`{server}__{tool}`）整体豁免**内建表
（operator 的 server id 不得与族词撞车；MCP 延迟有自己的旋钮 `[tools] defer_mcp_tools`）；
`media_understand`（多模态理解）在 chat（用户贴图）与 code（截图调试）均**刻意保留**——所以 media 用精确名不用族。

永不延迟的生命线：`tool_search` / `get_tool_schema` / `ask_user` / `session_set_mode` / `self_config`
（`NEVER_DEFER`——延迟发现机制本身会把模型困死在分区里）。`session_set_mode` 同时**常驻默认核**
（moa 先例：R8 一步开关不藏在 get_tool_schema 往返后面）。`tool_search` 在 deferred 层非空且披露启用时
由 `run_loop/inner.rs` 推进本请求的 core 集——它注册于 schema 快照**之后**（collapsed-but-unsnapshotted
类第三成员，见 `default_core_tools` 不变量注释），折叠它会让模型被指向一次必然 miss 的快照查询。

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
`prompt_line()` + `subagent_prompt_line()`（子代理短版，无 session_set_mode 契约句；copy 与分区表
同文件，R9）+ 分区表（`CHAT_CORE_SUBTRACT`/`CODE_CORE_ADD`/`*_DEFER_FAMILIES`/`*_DEFER_EXACT`/
`NEVER_DEFER` + `matches_family` 词边界匹配 + `__` MCP 豁免）。`effective_core_tools` 的 Chat 减法
带**空核守卫**：configured 非空/非通配时减空回退 configured 原样——空集是下游"披露关闭"哨兵，
减空会让 chat 反而比 work 更肥（escape hatch 必须 operator 显式）。

**三孪生管道**（与 exec_tier / think_level 逐点同构）：

1. 载体入口：`chat.send`/`agent.run` 顶层 `mode` 参数（`SendParams.mode` → `AgentRunParams.mode`，
   `handlers/chat.rs` + `handlers/agent.rs`，双入口 `server_init.rs` 同步）；
2. 校验+stamp：`build_run_request`（`handlers/agent.rs`，未知 id fail-loud）→
   `RunRequest.metadata[MODE_SESSION_KEY]`；
3. 每轮解析：`src/gateway/execution_engine/turn_mode.rs::{resolve_turn_mode,resolve_session_mode}`
   （request > session > global，stamp-on-carry best-effort，malformed fail-soft；无 channel clamp——
   模式非安全边界）。**slash 快路径同源**：`slash_command.rs::slash_gate_reason` 在 tier 解析旁同跑
   mode/think 解析（取值弃用、只为 stamp-on-carry 副作用——快路径就是这一 turn，骑在 slash 消息上的
   模式选择不得静默丢失）；
4. 会话持久：`identity_meta.custom["session_mode"]`；`sessions.patch` 配对校验
   （`db_handlers/modify.rs`，null=清除跟随全局）+ 列表投影 `SessionInfo.mode`
   （`db_handlers/{query,types}.rs`）。

**分区落点**（`run_loop/inner.rs`）：`mode_core_tools = session_mode.effective_core_tools(...)`
喂进两处 `build_request_tool_service` 与 SchemaLookupTool 注册门；mode 延迟名并入
`DeferredTools` 构造集（与 `defer_mcp_tools` 同一 Arc，`ToolSearchTool` 晋升路径不变）；
ToolSearchTool 注册处把 `tool_search` 推进 `mode_core_tools`（披露启用时——快照豁免类第三成员）。
**子代理继承**：子代理经 `parent_view_for_children` 继承父 turn 的分区面，非 Work 模式时
`SubagentTool::with_session_mode` 沿 strategy 同缝把 `subagent_prompt_line()` 焊进子 prompt
（`SpawnRequest.session_mode` → `PromptBuilder::with_session_mode` 后置焊接；Work=恒等分区跳过，
子 prompt 字节等价）。

**prompt line**：`FlowRequest.envelope`（[`TurnEnvelope`](FEATURE_LOCATOR.md#23-context-模式-context-mode--codex-风格)，
与 `exec_tier` / `cwd` 同一结构体）→ `HarnessRunner::run` 参数 → `resolve_prompt_context` →
`ResolvedContext.session_mode` → **`OperatingEnvelopeLayer` @1758（Dynamic）**
（`Usage mode:` 行，紧邻 `Approval mode:` 行）。

> ⚠️ 2026-07-26 起这两行**不在** `SecurityLayer` @600。那层不覆写 `stability()`＝默认 **Stable**，
> 即**可缓存前缀**；而模式/档位是**每轮可翻的旋钮**（composer pill / `session_set_mode` /
> `self_config`）。第 40 轮翻一次 pill 就改写 Stable 区一个字节 → 整段会话的 provider 前缀缓存
> 作废（历史从 0.1× 读变 1.25× 写）。不能直接把 @600 翻 Dynamic（`stable_layers_come_before_dynamic`
> 禁止两区交错），故两行搬进 `## Operating Envelope`（`src/thinker/layers/operating_envelope.rs`）。
> prompt 总字节不变，只换缓存分区。加新的"每轮可变"提示行时**默认放这里**，别放 `SecurityLayer`。

**R8 工具**：`src/builtin_tools/sessions/set_mode_tool.rs`（`session_set_mode`，
镜像 `session_rename` 的 8 个注册点：definitions/groups/struct_def/constructor/
optional_tools/tool_registry_impl dispatch/sessions mod/别名无）。v2：描述携带
whenToUse 目录（chat=快问答 / work=通用多步 / code=repo 工程）+ "下轮生效、切后收尾当轮" 契约
+ "用户要求才调用、任务明显超出当前模式时可**建议**"（Roo whenToUse + Cline 提案语义收敛）；
并已入 `default_core_tools`（schema 常驻）。

**全局默认**：`PoliciesConfig.mode`（`[policies] mode = "chat|work|code"`），
`config.get_tool_permissions` 响应携带 `mode` + `modes`（与 tier 同一 fetch 同一解码器），
`config.update_tool_permissions` 接受 `mode` 部分更新。

**Panel**（`interfaces/webchat/`）：`views/chat/mode_picker.rs`（composer pill，exec_tier_picker
克隆；presets 空=旧核→整体隐藏）+ `components/mode_labels.rs`（i18n 显式 match，未知 id 降级裸 id）+
`ChatState.session_mode`（会话覆盖，5 生命周期位点 + SessionSnapshot）+ **`ChatState.global_mode`**
（全局默认镜像，picker fetch 喂入，全局态不随 clear_session 清空、不进快照）+ locales
`settings.policies.mode_*`（en/zh）。**载运纪律（v2 P1 修复）**：4 条 send 路径（typed / queue-flush /
听写 voice / 沉浸 voice）只在**首条消息**（`session_key` 为 None）载运 mode——会话存在后 store 权威，
每次重发 pill 缓存值会以 request 优先级**静默回滚**模型的 `session_set_mode` 切换；live 会话的 pill
切换直接走 `sessions.patch`。**回同步**：`chat_sidebar.rs` 的 Effect 监听会话列表刷新，把当前会话行的
mode/exec_tier 覆盖值回写 `chat.session_mode`/`session_exec_tier`（`session_set_mode` / 外部 patch →
`run.session_updated` → 列表刷新 → pill 与右栏读到真值）。**侧栏**：会话行显式覆盖时渲染 mode 徽标；
重选还原不变。**team chat**：mode/tier 两 pill 隐藏（team send 不携带、session_key 已清、写不进）。
**Settings→Policies**：全局 `[policies] mode` 三卡选择器（`ToolPermissionsApi::set_mode`，tier 孪生面板面）。
**右栏：模式不再分派（锚点订正 2026-07-27）**——此处此前写「`state/layout.rs::follow_plan` +
`events.rs` 的 `tool_call_started` 按有效模式分派（chat=不驱动 / work=有计划后 follow Plan /
code=follow 工具）」。那套 live-follow 是**上下文检查器**时代的东西，随 `components/inspector/`
在产物面板重构（§6.8）中整体删除，`follow_plan` / `follow_tool` 现在**在代码里一个都不存在**。
右栏是产物面板，内容＝「这次会话产出了什么」，与会话模式正交：模式只做**工具呈现面**的静态分区，
不改右栏。`events.rs` 的 `tool_call_started` 也不再碰右栏状态（2026-07-27 round-3 一并撤掉了它
残留的徽标计数，见 FEATURE_LOCATOR §6.8）。

**团队 run 钉 Work，不继承全局。** 成员 run 不是用户会话（无 composer、无 pill），
留空会落到全局 `[policies] mode`——`chat` 档把 `task`/`team` 整族 defer 掉，正好是
`teams::leader_prompt` 点名要 leader 调的四步动词。所以两个团队 run 产地
（`broadcast::member_run_metadata` 群聊、`dispatcher::runner::task_run_metadata`
任务派发/workflow step）都显式写入 `teams::run_mode::TEAM_RUN_MODE`。
不变量测试从 `member_provision` 的 `WORKER_ESSENTIAL_TOOLS` /
`LEADER_ESSENTIAL_TOOLS` 取数——声明侧契约与呈现侧分区因此不可能各说各话。
钉死压得住模型自己的切换：团队 run 里调 `session_set_mode` 会成功写进会话，
但每个团队 run 的 metadata 都重新 stamp 一次，`requested.or(stored)` 让
request 携带的 `TEAM_RUN_MODE` 继续赢过 stored 值——工具回报"已切换"，团队 run
的工具面却一轮都不会变。
**团队要收窄工具面用每成员 `tools` 声明，不是用 mode。**

## 4. 刻意不做 (Deliberately NOT)

0. **不做「规划模式 / plan mode / 只读模式」这第四档。** 这不是「以后再说」，是**结构性不可以**：
   本文件 §1 与 `session_mode.rs` 模块头都逐字承诺「模式不授予、不拒绝任何权限」，而
   `SessionMode::prompt_line()` **每一轮都把这句话发给模型**。一个会拒绝的模式让那句话在
   被读到的当场变假（判据 §0：一句关于「什么被闸住」的话有三份拷贝，最贵的那份是发给
   模型的）。只读规划是**第四根正交旋钮** `PlanPhase`，住在
   `src/config/types/policies/plan_phase.rs`，强制点在 `effective_permission` 的**地板位**
   （explicit `tool_permissions` 条目**之上**）——详见 [PLAN_HANDOFF.md](PLAN_HANDOFF.md)。
   要收窄工具**呈现面**才来这里；要收窄工具**权限**去那里。
1. 不绑 per-mode 模型（`select_model`/agent hint 已覆盖；session_model_handle 重启即失）；
2. 不动 PromptMode Full/Compact/Minimal（prompt_mode.rs 实测警告）；
3. mode 不携带/不联动 exec tier（两 pill 各自显示、互不写入）；
4. 不做 mode 的 channel clamp（模式非安全边界）；
5. 不做自定义模式（三内建；`session_mode.rs` 的声明式表为未来留形，零现实消费者不预建）；
6. 不做 per-mode compaction/context budget（无 per-run 缝）；
7. 不绑 `FlowOverrides.extra_system_prompt`/`context_mode`（零消费者死字段，CUT 候选）；
8. 右栏不建新 surface/tab 系统（只做模式条件默认行为，在 InspectorTarget 缝内）。

~~已知限制（v1）：Panel 右栏行为只认会话显式 override~~ **已修（v2）**：全局默认经
`ChatState.global_mode` 挂进事件处理层，右栏按有效模式（会话覆盖 else 全局）分派。

**v2 调研沉淀的 backlog（未做，机制已验证过 Aleph 约束）**：一次性 per-message mode
（request 载运不 stamp 的变体，Aider one-shot 形状——现协议 request 恒 stamp）；per-workspace
sticky mode（Claude Code per-folder 记忆形状）；work/code 会话一键旁支 chat 会话（Codex Quick
chat / /side）；transcript 密度档（Normal/Verbose/Summary 纯客户端渲染档，mode 给默认值）；
work 模式右栏 artifacts 台账（files-touched ledger，从工具事件推导、内容盲）；自定义模式
（config 声明的分区表行 {slug,resident±,defer_families,prompt_line}——三内建仍是唯一现实消费者）。

## 5. 排查话术 (Troubleshooting)

- 「为什么这个工具不在列表里」→ 先看会话模式：chat/code 的整族延迟是**软分区**，
  模型 `tool_search` 一次即晋升；exec tier 的 Deny 才是硬隐藏。
- 「模式切了没生效」→ 分区在**下一轮**重建（工具面 per-request 构建）；查
  `resolve_turn_mode` 的三级来源与 `identity_meta.custom["session_mode"]` 实值。
- 「模型切了模式又跳回去」→ v2 已修的 P1 类：Panel 只在首条消息载运 mode；若复发，
  查是否有新 send 路径重新携带了 pill 缓存值（request 优先级会覆盖并重 stamp store）。
- 「MCP 工具被模式收起了」→ 不应发生：`defers_tool` 对含 `__` 的限定名整体豁免；
  真要收 MCP 走 `[tools] defer_mcp_tools`。
- 新增工具族时：想让 chat/code 模式收起它，改 `session_mode.rs` 的族表/精确表**一处**即可
  （族=词边界匹配，想留族内个别成员就学 media：族改精确名枚举）；绝不在别处再建第二张表。
