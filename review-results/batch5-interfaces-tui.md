# 静态代码审查报告 — interfaces-tui

- 审查单元：`interfaces/tui`（TUI 客户端）
- 审查方式：无 diff 全量静态阅读（worktree 与 main 一致）
- 日期：2026-07-22

## 统计

- 文件数：24 个 `.rs`
- 总行数：6870 LOC（含测试；`app/tests.rs` 803 行、`markdown.rs` 内嵌测试约 200 行）
- 最大文件：`app/mod.rs` 685、`markdown.rs` 625、`commands.rs` 598、`keys.rs` 517
- 生产代码中 `unwrap()`/`expect()`：0 处（仅测试内 2 处）；`unsafe`：0 处；regex：无依赖、无使用
- rust-doctor `rd-interfaces.json`：文件非合法 JSON（空/损坏），且无任何 tui 条目，未采用

## 历史问题复核

| 历史问题（2026-07-20） | 现状 |
|---|---|
| `tui/cost.rs` 定价表在 shell 层 | **已修复**。`cost.rs` 不存在；`commands.rs:239` 的 `cost_line` 直接渲染 daemon 下发的 `session.usage.cost_usd`，注释明确"the TUI no longer owns any pricing (R4)" |
| `tui/app/trace.rs` 事件路由逻辑在 shell 层 | **部分残留，可接受**。`trace.rs` 仍在 shell 层做 `AgentTraceEvent → UI 状态` 投影，但呈现文本/参数摘要均委托 `aleph-protocol` 的 `present_agent_trace_event_with_preset` / `summarize_tool_input`（trace.rs:112-114, 186-189），shell 只持有渲染状态，无业务决策。判为 R4 合规边界内，不再作为问题报告 |

## 发现列表（按严重级排序）

无 Critical / High。

### Medium

**M1. trace replay 污染当前会话的 token 计数与 cache 统计**
`interfaces/tui/src/tui/app/trace.rs:220-223`（另 236-248，入口 `trace.rs:273-275` `load_trace_replay`）
`load_trace_replay` 逐条重放持久化 trace 时与 live 路径共用 `apply_agent_trace_event`。重放里的 `SessionCompleted` 会走 `update_total_tokens_from_trace`，把**已结束历史任务**的 `total_tokens` 累加进状态栏的当前会话计数；同理 `ProviderUsage` 会覆盖 `cache_stat`。用户只是查看一个 replay，状态栏 token 总量就被抬高。守卫位 `current_run_trace_summary_applied` 只防一次 run 内重复计数，不区分 live/replay。
建议：在 `load_trace_replay` 投影循环期间跳过计数/统计类副作用（如加 `replaying: bool` 标志，在 `SessionCompleted`/`ProviderUsage` 两个 arm 中提前返回），或重放结束后保存并恢复 `total_tokens`/`cache_stat`。

**M2. 聊天区每帧对全部消息重做 markdown 渲染（~20fps 全量重建）**
`interfaces/tui/src/tui/mod.rs:137,175` + `interfaces/tui/src/tui/widgets/chat_area.rs:42`
主循环 50ms tick 每次都 `terminal.draw`，`render_chat_area → build_all_lines` 对 `state.messages` 全量执行 `markdown_to_lines`（大量 String/Vec 分配）。会话变长后每帧 O（全历史） 分配与解析，空闲时也在烧 CPU；长会话下可感知的卡顿/费电。
建议：缓存渲染结果（按消息代际/宽度做 invalidation），或至少在状态未变且非 spinner 帧时跳过 draw。

### Low

**L1. `/undo` 的 `keep_count` 用本地消息数推算，与服务端历史可发散**
`interfaces/tui/src/tui/commands.rs:328-341`
`conversational_count` 来自本地 `state.messages`。若某次发送失败（`add_user_message` 先于 `send_to_agent`，失败的用户消息留在本地但服务端无记录）、或流式期间插入系统消息导致本地一个 turn 拆成多条 assistant，本地计数与服务端历史不一致，`session.truncate` 的 `keep_count` 会截错位置。
建议：由服务端按"最后一个 turn"语义截断（不下发绝对 keep_count），或 truncate 后用 `chat.history` 校准。

**L2. `Reasoning` 事件未按 agent-trace 模式去重，`ReasoningBlock` 却有**
`interfaces/tui/src/tui/app/events.rs:41-50` 对比 `179-186`
`ReasoningBlock` 在 `current_run_uses_agent_trace` 时直接丢弃，`Reasoning` 不做该检查。若 gateway 在 agent-trace 模式下同时下发 `Reasoning` 与 `AgentTrace::TextEmitted(Intermediate)`，推理文本会双份追加。是否发生取决于服务端事件派发（本次审查范围外未能验证），但两个相邻 arm 的不对称处理值得对齐。
建议：给 `Reasoning` arm 加同样的 `current_run_uses_agent_trace` 早退，或注释说明为何不需要。

**L3. 取消 run 存在两条 RPC 路径：`agent.cancel` 与 `chat.abort`**
`interfaces/tui/src/tui/mod.rs:243`（Ctrl+C 级联）vs `interfaces/tui/src/tui/commands.rs:303`（`/stop`）
两处都是"中止当前 run"却调不同 RPC。该项目历史上有过死 wire 先例（`agent.respondToInput`，见 mod.rs:334-336 注释），建议确认两者在服务端都存活且语义一致，否则统一为一个。

**L4. 超大文件（>500 行软上限）**
`app/mod.rs` 685、`markdown.rs` 625（含约 200 行测试）、`commands.rs` 598、`keys.rs` 517；`app/tests.rs` 803（纯测试，可豁免）。非阻断，后续触及这些文件时可顺势拆分。

**L5. `execute_local_command` 的 `textarea` 参数为死参**
`interfaces/tui/src/tui/commands.rs:43-99`，函数体末尾 `let _ = textarea;`。
建议：从签名移除，调用点（mod.rs:227, 306）同步去掉。

**L6. spinner 帧表重复定义**
`interfaces/tui/src/tui/widgets/status_bar.rs:18-21` 与 `tool_block.rs:14-17` 完全相同的两份 `&[&str]` Braille 帧表。建议提取到 `theme.rs` 或共享常量。

## 安全审查摘要

- **审批（Ask tier）链路**：`approval.rs` 的 `select_session_approval` 会话过滤是 load-bearing（模块注释明确 `exec.approvals.pending` 为全局、`exec.approval.resolve` 服务端无归属校验），过滤逻辑有针对性单测（approval.rs:160-198），覆盖他会话/缺 id/畸形响应。Esc 不会解散审批与 AskUser 对话框（keys.rs:100-120），不会孤儿化 parked run。服务端无归属校验本身是 gateway 侧隐患，超出本单元范围，但值得在 gateway 审查中跟踪。
- 无注入面：所有 RPC 参数经 `serde_json::json!` 构造；slash 解析为纯字符串匹配，无 shell 拼接、无正则。
- 无凭证/密钥处理、无网络直连（均经 `aleph-client`），无文件系统写入。
- panic 面：生产路径无 `unwrap/expect`；panic hook 恢复终端（mod.rs:72-78）；所有切片索引用 `get`/`saturating_*`/`try_from` 防护。

## 架构红线合规快照

| 红线 | 状态 | 说明 |
|---|---|---|
| R1 | N/A | 不触及平台 API |
| R2 | 合规 | TUI 为终端渲染，非复杂业务 UI |
| R3 | 合规 | 依赖均为终端 UI 必需（ratatui/crossterm/tui-textarea/textwrap），无重依赖 |
| R4 | 合规（改善中） | 定价已下移 daemon；trace 投影的呈现逻辑委托 shared/protocol；`commands.rs` 仍有响应结构定义与格式化，属协议映射/展示，可接受 |
| R7 | 合规 | Cargo.toml 明确禁止依赖 alephcore，仅经 WebSocket + aleph-protocol 通信 |
| R8 | 合规 | 无正则；意图路由全在服务端 |
| R9 | 合规 | `/tier` 等配置经 `sessions.patch` RPC，服务端校验 |
| R10 | N/A | shell 无 prompt |

## 结论

无 Critical/High。TUI 整体是干净的薄壳：审批安全过滤有测试、终端恢复完备、panic 防护到位。两项 Medium 均为逻辑/性能正确性问题（replay 污染计数、每帧全量重渲染），建议优先处理 M1。
