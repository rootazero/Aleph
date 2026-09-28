# Track A Findings — 消息渲染 / 对话展示

> **Spec 关联**：`docs/superpowers/specs/2026-09-21-panel-tui-polish-design.md` §3 A（A1–A7）
> **时间**：2026-09-21（worktree panel-tui-polish @ 5e1b29062）
> **关联 baseline**：`0927461ba`（spec §1）
> **L1 范围**：仅修 bug + 补连线 + 补测试；不重写大块模块、不抽新公共面（spec §7）

---

## 1. FEATURE_LOCATOR 已知未完成项（Panel / TUI 消息渲染相关）

| 口语关键词 | Anchor | 状态 | 对应 A1–A7 | 备注 |
|------------|--------|------|------------|------|
| Panel Reads `terminate_reason`（含 halt 文案 + i18n） | `views/chat/state/mod.rs::RunHalt` + `events.rs::parse_run_halt` + `messages.rs::halt_view` + `locales/*.json::chat.halt_*` | ✅ (§3.17d④, 2026-08-29) | **A6** | Panel 端面面齐备；TUI 走 `terminate::effective_token` + `halt_notice`（同表）—— **跨端一致**（§3.17e⑤：5 面收敛为同一张表） |
| Streaming Echo & Workspace Panel | `timeline.rs` + `messages.rs::MessageList` + `events.rs::reconcile_tools`/`settle_orphan_tools` + `state/mod.rs::TOOL_STATUS_UNKNOWN` + `tool_card.rs` + `state/typewriter.rs` | ✅ (§6.1，6 轮 round 至 2026-08-26) | **A1·A2·A7** | 打字机收尾 lag_floor、reconcile 末态、ExploreGroup ✓→Outcome 均已收敛；本轮重点是「保不破」而非新功能 |
| Cross-Session Frame Guard (TUI) | `tui/app/events.rs::frame_belongs_here` + `app/mod.rs::session_reconciled` | ✅ (§6.9 round-2, 2026-08-23) | **A7** | TUI 跨会话防串台已落地 |
| TUI Dual-Projection Fix | `tui/app/{events,trace}.rs::turn_streamed_len` + `reconcile_tools_from_summary` | ✅ (§5.13, 2026-08-07) | **A2** | 流式回显两路收敛已稳定 |
| Reasoning Panel (折叠 + live preview) | `views/chat/reasoning.rs`（collapsible `ReasoningCard` 移植）+ `state/mod.rs::reasoning_text` | ✅（2026-08 月新增，§6.1 round-6 顺带） | **A3** | Panel 有；TUI 的 `Reasoning` 走 `TranscriptEntry::Reasoning` + 仅 `/verbose` 显示 —— 语义不同（见 §4.3.a） |
| Tool Row Folding（CC 风格 ⏺ Tool(args) + ⎿ 输出，3 行物理封顶） | TUI `tui/widgets/tool_row.rs`（`render_tool_row` + `FoldPolicy::for_display_name` + 共享 `shared_ui_logic::transcript::fold`）+ Panel `tool_card.rs` + `timeline.rs::ExploreGroupRow` | ✅（TUI 已完整；Panel 走卡+折叠块） | **A1** | 物理行封顶、unavailable diff、toggles 矩阵均已测 |
| Tooltip 行（带 ⏺ Tool(args) + ⎿ 输出，3 行物理封顶） | TUI `tui/widgets/tool_row.rs` 同上；Panel `messages.rs::ExploreGroupRow` | ✅ | **A1** | 物理行封顶、unavailable diff、toggles 矩阵均已测 |
| Attachment Composer（拖拽 / 粘贴 / paperclip） | Panel `composer/attachments.rs::read_file_list_into` + `AttachmentPreviewBar` + `state::PendingAttachment`（**仅 composer**）；TUI **无 attachment 流** | ⚠️（仅 Panel composer；user bubble **不显示已发送图片**） | **A4** | **核心 gap**：见 §2.A4 + §3.b |
| User Bubble 内的 Attachment / Image 渲染 | Panel `messages.rs::MessageBubble` —— user 臂**只有**`{content}` 文本，无 `<img>` / `<ImageLightbox>`；TUI `TranscriptEntry::UserText` 只有 `text: String` + `at_ms`，**没有 attachment 字段** | ❌（**两端均缺失**） | **A4** | **核心 gap**：见 §2.A4 + §5 |
| Message-action bar（hover / click / menu） | Panel `messages.rs::MessageBubble`（hover action bar：clock / Copy / Retry，`group-hover:opacity-100`，已经过守卫 `every_wrapper_around_a_bubble_declares_a_definite_width`） | ✅（已在 §6.1） | **A5** | TUI 走 click-toggle RegionTable + Header 复位（regions.rs）—— 平台语义差异但等价 |
| `RunHalt::label` 与 `aleph_protocol::terminate::label` 跨端共用 | `aleph_protocol::terminate::label` + TUI `app/events.rs::halt_notice` + Panel `state/mod.rs::RunHalt::label` | ✅（§3.17e⑤ 5 面收敛） | **A6** | 共享同一张表，措辞逐面分持 |
| `halt.detail` 优先于 `halt.reason` | `RunHalt::label` 用 `self.detail.unwrap_or(&self.reason)`；TUI `terminate::effective_token(detail, reason)` | ✅ | **A6** | 一致 |
| 跨端展示 `terminate_reason`（A6 是「守住不破」） | TUI `halt_notice`（i18n 双语，`halt_notice(token, locale)`）；Panel `halt.label(i18n.get_locale())` | ✅ | **A6** | 双语覆盖，跨端同源 |
| Reconcile tools from summary | Panel `events.rs::reconcile_tools` + `backfill_tool_errors`；TUI `app/events.rs::reconcile_tools_from_summary` | ✅（两端共用 `summary.tool_summaries`） | **A7** | 末态对账均依赖核心权威摘要 |

> **关键观察**：A1 / A2 / A3 / A6 / A7 ✅ 的「不破」线已被 FEATURE_LOCATOR 6 轮 round 至 2026-08-29 全部覆盖；**唯一真正缺口**集中在 **A4（用户消息的图片/附件渲染）**，且缺口呈「**两端都缺**」形态，详见 §2.A4 + §5。

---

## 2. Spec §3 A 子项覆盖矩阵

| 子项 | Aleph 锚点 | 现状 | 与参考项目差距 | Tier 候选 |
|------|-----------|------|----------------|-----------|
| **A1** tool 行折叠（长 JSON 不刷屏） | Panel: `messages.rs::ExploreGroupRow`（1 项降级为 `ToolLine`，§6.1 R5）+ `tool_card.rs`；TUI: `widgets/tool_row.rs::render_tool_row` + `shared_ui_logic::transcript::FoldPolicy` + 物理行封顶 3 行 + `diff_rows`（unavailable 不伪装为空 diff） | ✅ 双端共用 `fold` / `FoldPolicy` / `diff_rows` / `stats_label`（§6.1 R5）；物理行 vs 逻辑行严格区分（`the_hint_counts_physical_rows_not_logical_lines` 已红→绿） | pi-CC 的 `⏺`（U+23FA）单字符 + `⎿`（U+23BF）TUI 已落地；`diff_rows` unavailable 分支显式「不为空 diff 装样子」是 CC 原版没有的硬化 | **没有**（✅ 已稳） |
| **A2** streaming echo 中断恢复 | Panel: `events.rs::apply_trace_event` + `RunSummary.tool_summaries` 权威对账 + `messages.rs::MessageList` `history_has_more` + `load_earlier` 分页；TUI: `app/events.rs::reconcile_tools_from_summary` + `settle_orphan_tools` + `app/mod.rs::adopt_active_run`（`hydrate_and_follow` 端） | ✅（§6.1 R1「agent_trace 是有意有损，`tool_summaries` 是权威真相」；`run_complete` 走 `reconcile_tools` + `settle_orphan_tools`；`run_error` 与 `replay_run` 只跑 settle；attach 时 `restore_from` + `adopt_active_run`） | pi-CC 的「中断 + 重启后内容完整恢复」**端到端**未在 Aleph 测过；只能从代码读出已实现（`RestoreFrom` + `history_limit` + `HISTORY_PAGE`） | **Tier-2**（e2e 护栏缺失 —— Phase 3 加 Playwright 护栏后即可拆出） |
| **A3** reasoning 折叠状态在重连后丢失 | Panel: `state/mod.rs::reasoning_text` 单一信号 + `reasoning.rs::expanded`（**卡内本地 RwSignal**）；TUI: `TranscriptEntry::Reasoning { collapsed: bool }`（**结构性字段**） | ⚠️ **两边形状不同**：Panel `expanded` **没有跨对话存活**（attach → `restore_from` 重建信号，重置为 false）；TUI 字段是 **wire-level** 的（被 `restored without collapsed=true` 后才能持久化） | pi-CC：reasoning **默认折叠**，无展开态保留概念（每次都折叠） | **Tier-3**（与 pi-CC 路径相异；要不要让 Panel 用 `chat.history` 回灌 `reasoning.expanded` —— 这需要新增 wire 字段，**超出 L1**） |
| **A4** 附件/图片渲染（无 alt / 无尺寸 / 超长图） | Panel composer: `state::PendingAttachment { name, mime_type, data_base64, size }`（仅 send 前）；Panel 渲染: `messages.rs::MessageBubble` user 臂**只**显示 `m.content`（**不显示已发送的图片**）; TUI: **没有 attachment 字段**，`TranscriptEntry::UserText { text, at_ms }` —— **零 attachment 流** | ❌ **两端都缺**：① composer **送出后**  ② Panel user bubble 的 `m.content` 是 send 后 wire 序列化值，**没有图片渲染路径**；③ TUI 完全没有 attachment wire shape | desktop-cc-gui `MessageImages.tsx`：data URL 直接渲染；file path 通过 `ipc.read_file` 读 data URL；不可读 → 退化为文件名 chip；click → `ImageLightbox` 弹层；panel TUI Panel 共用通用 `user_idle_seconds` 是另一个事 | **Tier-1**（用户发图看不到图，**最直观**） |
| **A5** 行间 hover / click / menu（Panel） | Panel: `messages.rs::MessageBubble` `action_class`（`group-hover:opacity-100` + Copy + Retry 按钮 + 时钟）；wrapper 宽度守卫 `every_wrapper_around_a_bubble_declares_a_definite_width` 防止百分比塌陷；TUI: `widgets/regions.rs::RegionTable` + `widgets/tool_row.rs::RowRender.toggles`（header + 每一行 hint 各一 toggle） | ✅（Panel hover bar 已在 §6.1 落地；TUI click region 已 wired） | desktop-cc-gui 的 `MessageAnchorRail.tsx`、`CollapsibleMessage.tsx`、`ToolPayloadViewer.tsx` —— **远超 L1**（多消息锚点导航、payload viewer、collapse 动画） | **没有**（✅） |
| **A6** halt / terminate_reason 显示（守住不破） | TUI: `app/events.rs::halt_notice(token, locale)` + `terminate::effective_token`；Panel: `state/mod.rs::RunHalt::label(locale)` + `messages.rs::halt_view`（`title=format!("terminate_reason: {}", halt.reason)`）；`aleph_protocol::terminate::label` **单一表** | ✅（§3.17e⑤：5 个面 收敛成一张表）；Panel + TUI + CLI (`aleph exec`) + `aleph watch` + `reply_emitter::streaming::cap_notice_for` 都吃同一张表 | 跨端一致；`terminate::label` 提供本地化兜底（13 个 token + 未识别 → 原样返回） | **没有**（守住即可） |
| **A7** 跨端一致投影 | Panel + TUI + CLI 共用 `aleph_protocol::terminate::*`；共 wire `summary.tool_summaries`、`summary.plan`、`run_state.halt_*`；`events.rs::parse_run_halt` + `events.rs::parse_tool_settlements` 走同一 `unparse_run_halt` 双视图语义（§3.17d④） | ✅（§3.17 5 轮 + §6.9 7 轮；只剩一处遗留见 §3.a） | pi-CC：跨端概念不适用（单端）；desktop-cc-gui：React 单端 | **没有**（守住即可；新发现的细节见 §3.a） |

---

## 3. 潜在 bug 热点

> 全部针对 **生产代码**（不含 `#[cfg(test)]` / `#[allow]`）。Tier 候选：1（用户立刻可见）/ 2（功能性影响，间接可见）/ 3（结构/未来风险）。

### 3.a Panel `halt_view` 在 user 气泡中残留的状态查询（跨端一致性 L1 边缘）

- **位置**：`interfaces/webchat/src/platform/wide/views/chat/messages.rs:880`
  ```rust
  let run_for_halt = message_run_id.clone().unwrap_or_default();
  ...
  let halt_view = move || {
      if is_user() { return None; }            // ← guard against user bubbles
      let halt = chat.run_halts.with(|m| m.get(&run_for_halt).cloned())?;
      ...
  };
  ```
- **现象**：`run_for_halt` 对 user bubble 是 `""`（`unwrap_or_default()`），但 `is_user()` 提前返回 None —— **没问题**，但 `run_for_halt` 的派生本身就是 dead-ish code（user 永远读不到 map[`""`]）。如果未来 user bubble 形态改变（譬如在 user 旁附加发送回执），这里会静默误读。
- **修复方向**：把 `run_for_halt` 与 `is_user()` 合并成一个 `memo for halt_view`，user 时跳过派生；中（20–40 行）。
- **Tier**：3（现状不破）。

### 3.b TUI `add_user_message(content: String)` 丢掉了 attachment 信息

- **位置**：`interfaces/tui/src/tui/app/mod.rs:1144`
  ```rust
  pub fn add_user_message(&mut self, content: String) {
      let id = self.next_entry_id();
      self.messages.push(TranscriptEntry::UserText {
          id,
          text: content,
          at_ms: as_ms(Utc::now()),
      });
      self.sends = self.sends.saturating_add(1);
  }
  ```
- **现象**：`TranscriptEntry::UserText` 的 `text: String` 是 **wire-level `ResponseChunk` 的 content**，**没有 attachment slot**。`add_user_message` 也没有 attachment 参数。
- **修复方向**：要让 TUI 显示图片，需在 `TranscriptEntry::UserText` 加 `attachments: Vec<TuiAttachment>`（仅 name + mime，因为 ratatui 不显示图像本体）—— 但 wire 形状需要 TUI 客户端 DTO 反序列化，**超出 L1 范围**（D 赛道也是「不抽新公共面」）。
- **Tier**：2（结构性，**先 A4-5 而后这里**）。

### 3.c Panel `MessageBubble` user 臂直接渲染 `m.content`，**不**经过 markdown 解析

- **位置**：`interfaces/webchat/src/platform/wide/views/chat/messages.rs:1246-1254`
  ```rust
  let content = message.with_untracked(|m| {
      m.as_ref().map(|m| m.content.clone()).unwrap_or_default()
  });
  view! {
      <div class="whitespace-pre-wrap break-words text-sm leading-relaxed">
          {content}
      </div>
  }.into_any()
  ```
- **现象**：user 消息**不会**触发 markdown 渲染 / 代码块 / 链接 / 图片。`composer/attachments.rs` 已发送的附件，落到 wire `m.content` 后，**user bubble 不显示图片**。**A4 直击此处**。
- **修复方向**：要么 `message.content` 端区分「text vs image ref」走不同渲染分支；要么按 `desktop-cc-gui` 模式 —— `MessageImages` 组件解析 attachment 字段（**需要先在 wire 上加 image ref**）—— **后者是 wire 协议改动，超 L1**。
- **Tier**：1（用户能立刻看到的问题）。

### 3.d TUI `events.rs::halt_notice` 使用 `UiLocale::from_env()` **而不**传当前 UI locale

- **位置**：`interfaces/tui/src/tui/app/events.rs:542`
  ```rust
  self.add_system_message(halt_notice(token, UiLocale::from_env()));
  ```
- **现象**：`UiLocale::from_env()` 在启动期冻结为环境变量值；TUI 内 `/locale`（若存在）切换**不影响**已发送系统消息的 locale。已发送消息保留启动期 locale，但**新出现的**消息也用启动期 locale（**事件源是同一变量**）。
- **修复方向**：传 `state.locale()`（已有）；中（< 20 行）。验证后改 `halt_notice` 接 locale。
- **Tier**：2（用户能在调试时看到，但本地化不全）。

### 3.e Panel `MessageList` `stuck_to_bottom` 阈值 64px 与 Tailwind 巨大字号不匹配

- **位置**：`interfaces/webchat/src/platform/wide/views/chat/messages.rs:227`
  ```rust
  const STICK_THRESHOLD_PX: f64 = 64.0;
  ```
- **现象**：常量写死 64 px；accessibility 设置「字体放大」后 64 px 可能覆盖半屏内容。
- **修复方向**：改为相对视口高度 `0.25 * client_height`；小（< 20 行）。
- **Tier**：3（结构性）。

### 3.f TUI `clip_to_width` 用 `UnicodeWidthChar::width(ch).unwrap_or(0)` 静默丢宽字符

- **位置**：`interfaces/tui/src/tui/widgets/chat_area.rs:369`
  ```rust
  let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
  ```
- **现象**：`UnicodeWidthChar::width` **永不返回 None**（函数签名 `fn width(self) -> usize`）—— **`unwrap_or(0)` 是 dead code**，但意味着任何 CJK / Emoji 字宽异常（实测不会发生）会**静默**缩成 0 列，文本会重叠。
- **修复方向**：要么改用 `width()` 直接取（**永远是 usize**），要么加注释说明「防止 None 静默吞 char」。小（< 10 行）。
- **Tier**：3（实际不破；防御性）。

### 3.g TUI `MessageBubble` 不存在（Panel 专有），TUI 与 Panel 命名不一致

- **位置**：TUI 端无对应组件。
- **现象**：TUI 走 `TranscriptEntry` 枚举匹配 + `render_settled_message`；Panel 走 `<MessageBubble>` 组件。两个端走的是**完全不同**的渲染策略。
- **修复方向**：不可在 L1 统一（spec §7「不抽新公共面」）。
- **Tier**：不列（结构性，不属于 L1 修复）。

### 3.h Panel `add_user_message` 入参 `Vec<PendingAttachment>` 在 seed_draft 已有，但 user **send** 路径的最终 wire 不带 attachment 显示信息

- **位置**：`interfaces/webchat/src/platform/wide/views/chat/state/mod.rs:1040`（`seed_draft`）+ `composer/attachments.rs`（`PendingAttachment` 数据）
- **现象**：`PendingAttachment { data_base64, mime_type, name }` 通过 composer 路径送入；wire 序列化进 `m.content`（**仅文本**），**不保留 image ref**。`/send` 路径的 `pending_attachments` → 队列 → `drain_queued` 把 base64 嵌入 message 但不写 wire shape。
- **修复方向**：让 `ChatMessage` 增加 `attachments: Vec<AttachmentRef>` 字段（image ref）；**wire protocol 改动**，**超 L1**。
- **Tier**：2（结构性，**A4 子项**）。

### 3.i TUI `chat_area.rs::render_chat_area` `header.iter()` 之后 `out.extend(header[start..body_hi].iter().cloned())` — `body_hi - start` 行 slice 在 `header_height > 0 && start < body_hi` 时仍能裁空

- **位置**：`interfaces/tui/src/tui/widgets/chat_area.rs:687-695`
- **现象**：略复杂的负数边界测试；如果 `header_height == 1` 且 `start == 0`，`header[0..0]` 长度 0，正常；如果 `start == header_height - 1`（= body_hi - 1），切到空 slice 然后再 `out.push(Line::default())`，**逻辑正确**。
- **修复方向**：无需修；测试覆盖。
- **Tier**：不列。

### 3.j Panel `messages.rs::MessageBubble::on_retry` 在 reactive 闭包外捕获 `chat`

- **位置**：`interfaces/webchat/src/platform/wide/views/chat/messages.rs:1108`
  ```rust
  let on_retry = move |_: web_sys::MouseEvent| {
      chat.request_retry();
      ...
  };
  ```
- **现象**：`chat` 是 `expect_context::<ChatState>()`，**反应式上下文**；通过 `move` 闭包捕获**一次性值**。`request_retry` 内部读最新 chat 状态（实参），所以 OK。**没有泄漏**，但模式上不像 Leptos 推荐的 signal 捕获。
- **修复方向**：无需修；现有测试覆盖。
- **Tier**：不列。

### 3.k Panel `messages.rs::run_id_from_message_id` 解析 `intermediate-run-x-7` 给出 `run-x`（手卷格式）

- **位置**：`interfaces/webchat/src/platform/wide/views/chat/messages.rs:1623-1627`
  ```rust
  #[test]
  fn strips_assistant_and_intermediate_prefixes() {
      assert_eq!(run_id_from_message_id("assistant-r1"), "r1");
      assert_eq!(run_id_from_message_id("intermediate-r1-3"), "r1");
      assert_eq!(run_id_from_message_id("intermediate-run-x-7"), "run-x");
      ...
  }
  ```
- **现象**：若 run id 含连字符（`run-x-7`），`intermediate-run-x-7` 解析成 `run-x`，**丢掉末尾 `-7`**。生产端如何命名 unknown；假定 run id 内部不含连字符。
- **修复方向**：若 run id 格式不稳定，加 `split_once('-')` + `nth_tail` 拆；小（< 10 行）。
- **Tier**：3（结构风险，与 §6.1 命名一致性相关）。

---

## 4. pi-claude-code-tui 模式差距

| pi-CC 模式 | Aleph 当前实现 | 差距 | 是否 L1 | Tier |
|------------|----------------|------|--------|------|
| `⏺ Tool(args)` 单字符 glyph | TUI 走 `\u{23fa}`（**也是**单字符；CC 用 `\u{23fa}`，pi 复刻亦同）；已测 `the_status_glyphs_are_all_one_column` | **无差距**（§tool_row.rs 已严格保证 glyph 一列宽） | — | — |
| `⎿  输出` 两列字符 | TUI `BODY_FIRST = "  \u{23bf} "`（4 列含 2 空格 + glyph + 1 空格）；`BODY_INDENT = 4` | **无差距**（宽度固定） | — | — |
| CC 工具行**物理行封顶 3 行**（spec §1 "3 行折叠"） | TUI `FoldPolicy::for_display_name` + `fold` + `spinner_frame` + `stats_label`；共享 `shared_ui_logic::transcript::fold`；物理行而非逻辑行（`the_hint_counts_physical_rows_not_logical_lines` 已 red→green） | **无差距**（物理行封顶是 §6.1 R5 hard；TUI 端已落 + 测） | — | — |
| 错误红色 + edit 彩色 diff + read 折叠摘要 | TUI: `SemanticColor::ToolErr` + `diff_rows` 双色 + `for_display_name().body == FoldBody::Hide` 给「Read 120 lines」摘要 | **无差距**（TUI 端已落 + 测 `a_failed_shell_keeps_the_tail`） | — | — |
| Spinner `✻` 花型动画 + Claude 橙 + 190 动词 + (esc to interrupt · Ns) | TUI: `shared_ui_logic::transcript::SPINNER_FRAMES`（与 CC 的 BLOSSOM 同源）+ `SemanticColor::ToolRunning`；Panel: `animate-pulse` | **部分差距**：TUI 端有 SPINNER 帧，**Panel 端用 CSS animate-pulse**（无花型 glyph）；CC 风格的 `(esc to interrupt · Ns)` 仅出现在 `TUI hint_line`，**Panel 不显示**（hover 才显示 timestamp + Copy + Retry） | **Tier-3**（不在 L1） |
| 收尾 `✻ Worked for 12s`（CC 过去式动词） | TUI: `turn_summary_text(summary)` 在 `TranscriptEntry::TurnSummary`；Panel: `cost_view` 在 `MessageBubble` 显示总耗时 | **部分差距**：TUI 完整；Panel 走 cost 行无 ✻ glyph | **Tier-3** | 不列 |
| 第三方 / MCP 工具兜底（凡无 renderCall/renderResult → 折叠 CC 行） | Panel: `tool_card::ToolKind::from_name(name)` + `tool_icon` 已认 web_fetch / skill / memory glyph；TUI 走 `RowBody` 枚举，**所有**工具统一渲染 | **无差距**（TUI 兜底更统一） | — | — |
| 长 JSON 行 wrap 后**不**刷屏（3 行物理封顶） | TUI: `FoldPolicy::collapsed_rows`（默认 3） + `textwrap`（markdown） | **无差距**（TUI 物理行封顶严格） | — | — |

---

## 5. desktop-cc-gui 模式差距（仅消息相关）

| desktop-cc-gui 模式 | Aleph 当前实现 | 差距 | 是否 L1 | Tier |
|--------------------|----------------|------|--------|------|
| `MessageImages.tsx` (data URL 直渲染，file path 经 IPC 读 data URL；不可读退化为文件名 chip；click → ImageLightbox) | Panel: **完全无对应** —— `MessageBubble` user 臂只渲染文本 `m.content`；`composer/attachments.rs::PendingAttachment` 只**发送前**存在 | **核心差距**：用户拖入图片后**看不到图**（A4） | 是（修法 wire-level，**超 L1**） | **Tier-1**（**列入下轮**，本轮可做的子集：① 把 composer 的 `PendingAttachment` 渲染为「sent-image」chip —— chip 上显示文件名 + 大小，**不是**真实图，但能告诉用户「这张图被发出去了」；② 给 Panel 加 `image-only-chip` 渲染：若 `m.content` 含 `data:image/...` substring，给 chip 占位；③ TUI 端在 `TranscriptEntry::UserText` 加 `attachments: Vec<TuiAttachmentName>` —— 也只显示文件名，**不**显示图（TUI 渲染图无意义）。**估计行数（小，< 40 行 / 端）**） |
| `MessageAnchorRail.tsx`（消息锚点导航） | Panel: 无；TUI: 无 | **结构性差距**（跨消息引用 + 跳转） | **否**（重构级，**超 L1**） | 不列 |
| `CollapsibleMessage.tsx`（消息级折叠 / 动画） | Panel: `MessageBubble` 整体常驻，tool_calls 子折叠在卡内；TUI: `ToolGroup.expanded` 控制组级折叠 | **基本覆盖**（粒度不同：Panel 是「工具卡」折叠，TUI 是「工具组」折叠） | — | — |
| `ProcessDisclosure.tsx`（工具调用 step-by-step 折叠 + file chip） | Panel: `tool_card.rs::tool_headline` + `tool_card.rs::file_chip_for`（**仅 Composer 一侧**）；TUI: `render_tool_row::file_changes_body` 显示文件路径 | **基本覆盖**（chip 在 Panel 不显示已发送文件名） | 是（小，< 30 行） | **Tier-3** |
| `toolTypeKey` 分类（web / shell / read / edit / search / task） | Panel: `ToolKind::{WebShell,Read,Edit,Search,Task,...}`；TUI: `ToolRow.body` 枚举 + `RowBody::{Text,FileChanges,None}` | **基本覆盖** | — | — |
| `RunStatusStrip.tsx`（run 终止状态条） | Panel: `messages.rs::halt_view`（小警告 + 终止 reason）+ cost 行 + token 行；TUI: `halt_notice` 加 system message | **基本覆盖**（Panel 是 in-bubble；TUI 是 system row —— **视觉差异**） | **否**（这是 §6.1 §3.17 6 轮收敛结果） | **守住不破** |
| `MessageTimeline.tsx`（消息列表时间锚） | Panel: `messages.rs::format_clock`（行级 + day-separator）；TUI: `render_user_message` 行级时钟 | **基本覆盖** | — | — |
| `QuestionCard.tsx` / `QuestionDock.tsx`（ask_user 卡片） | Panel + TUI 都有；统一走 `aleph_protocol::AskUserQuestion` + `render_ask_user`（TUI `app/events.rs`） | **基本覆盖** | — | — |
| `ScrollToBottomButton.tsx` | Panel: `messages.rs::on_jump` + pill；TUI: `BackToBottom` region（`regions.rs::RegionKind::BackToBottom`） | **基本覆盖** | — | — |
| `use-scroll-follow.ts`（吸底） | Panel: `shared_ui_logic::state::chat_scroll::scroll_action`（7 条宿主测试，§6.1 R2 R3 修复 peer 误读 + sweep 跟随）；TUI: `scroll_offset = 0` 跟随 | **基本覆盖** | — | — |

---

## 6. 范围闸

- **L1 范围（仅修 bug + 连线，不重写）**：Y
- **不在 spec 范围（要移到下轮）**：
  1. **A4 真图片渲染**（wire protocol 改动：`ChatMessage.attachments: Vec<ImageRef>`；`shared/protocol::ChatMessage` 新增字段 + TUI `TranscriptEntry::UserText` 新增 `attachments`；需要 §6.1 §3.17 收敛原则保持「不抽新公共面」—— 实际是「抽**新** wire field」，**超 L1**）。本轮可做的子集已列在 §5 第一行。
  2. **A3 reasoning 展开态跨会话持久化**（wire protocol 改动，**超 L1**）。
  3. **pi-CC spinner glyph**（`✻` + 190 动词 + (esc to interrupt · Ns)）—— 不在 A1–A7 范围，spec §1 「参考模式来源」不强制移植。
  4. **MessageAnchorRail**（desktop-cc-gui 重构级）—— **超 L1**。
  5. **跨端 `terminate::label` 文案对账**（§3.17e⑤ 已收敛成单表；本轮**守住不破**即可）。
- **Tier-1 候选汇总**（用户能立刻看到）：
  1. **A4**：user 消息的附件 / 图片渲染（Panel + TUI 两端都缺；最直观）。
- **Tier-2 候选汇总**（功能性影响）：
  1. **A2**：streaming echo 中断恢复的 e2e 护栏（代码已实现；缺 Playwright 护栏）。
  2. **3.d** TUI `halt_notice` 不传 `state.locale()`（小，< 20 行）。
  3. **3.b** / **3.h** TUI `add_user_message` 丢 attachment 信息 + Panel wire 不带 attachment 显示信息（A4 子项，先有 Tier-1 决策后做）。
- **Tier-3 候选汇总**（结构 / 未来风险）：
  1. **A3** reasoning 折叠展开态跨会话持久化。
  2. **3.a** Panel `run_for_halt` 与 `is_user()` 合并。
  3. **3.e** Panel `STICK_THRESHOLD_PX` 改相对视口。
  4. **3.f** TUI `UnicodeWidthChar::width` 永不 None 但 unwrap_or(0)。
  5. **3.k** Panel `run_id_from_message_id` 多连字符风险。
- **不在 Tier 候选**（结构性 / 守住不破）：A1、A5、A6、A7、3.g、3.i、3.j、A2 代码层（已实现；缺 e2e 护栏）。
- **估计行数总览**（仅 Tier-1 + Tier-2）：< 60 行；不破模块边界。