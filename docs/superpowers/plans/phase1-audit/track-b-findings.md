# Track B Findings — Composer / 输入交互

> **Spec 关联**：2026-09-21-panel-tui-polish-design §3 B（B1–B8）
> **基线 commit**：`0927461ba`（worktree HEAD `5e1b29062`，基于 `0927461ba`）
> **时间**：2026-09-22，Phase 1 审计扫描
> **L1 范围**：仅修 bug + 补连线 + 补缺失测试
> **关联 baseline**：Phase 0（`558509811`，alephcore 19k+ green + 3 pre-existing failures documented）

---

## 1. FEATURE_LOCATOR 已知未完成项（Composer / 输入 / ask_user 相关）

| 口语关键词 | Anchor | 状态 | 与 B1–B8 的对应 | 备注 |
|---|---|---|---|---|
| 横切 LLM 与用户互动 / 确认 / 授权 | `src/clarification/` + `src/builtin_tools/ask_user.rs` + `src/exec/manager.rs` + `src/exec/socket.rs::to_outcome` | ✅ §5.3 第五轮 2026-08-05 | B7 | 已全面接入（带 `RetireOnAbandon` RAII、`withheld_secret` 分区） |
| Clarification Cancel Terminal Frame / 僵尸 ask 卡 | `clarification/ask.rs::RetireOnAbandon` + `session.rs::cancel_abandoned` | ✅ §5.3 第八轮 2026-08-15 | B7 | 守卫测试 `dropping_a_parked_ask_retires_its_entry` 钉住；但 module 只有 1 个 `#[test]` + 9 个 `#[tokio::test]`（共 10），密度偏稀（见 §3 #4） |
| TUI overlay 仍是裸 deny（TUI ask / approval 的 Deny-with-reason 输入缺失） | `interfaces/tui/src/tui/approval.rs` | ⚠️ 显式延后 | B6 | FEATURE_LOCATOR §5.3 第六轮注释："TUI overlay 仍是裸 deny——但 2026-08-12 起 clarification overlay 有了文本编辑态，那条没有编辑态的理由已经不成立" → 基建已就位 |
| TUI ask_user answer 改走 `clarification.resolve`（Round-5 W1 修复） | `interfaces/tui/src/tui/keys.rs::RespondToDialog` | ✅ 2026-08-08 | B7 | wire 协议 `StreamEvent::AskUser` 已带 `session_key`；DIALOG overlay 的 Tab 切 menu/typing、Esc 不可逃 |
| Panel composer auto-grow Effect 对每一次程序化重写都失效（Round-7 ⑨ 已修） | `composer/mod.rs:172` 起 `if ta.value() != text { ta.set_value(&text); }` | ✅ 2026-08-03 | B3 | 注释精确解释了「DOM 先变 vs 信号先变」的镜面问题，typed 路径已通过 value 比对再写 |
| `draft_seed` 预填通道整体退休（Round-7 ③） | `composer/mod.rs:716-720` 注释 | ✅ 2026-08-03 | B1/B7 | 单一入口 `ChatState::seed_draft`，合并而非覆盖；phone composer 不再有零消费者 |
| `voice_mode` 同左 file-drop zone（G5）尚为预留 | `composer/attachments.rs:7` "chat-surface drop zone (the latter not yet wired through here)" | ⚠️ 显式延后 | B4 | 当前只有 paperclip `<input type="file">`，无 `dragover` / `drop` / 粘贴图片分支 |

---

## 2. Spec §3 B 子项覆盖矩阵

| B 子项 | Aleph 锚点 | 现状 | 与参考项目差距 | Tier 候选 |
|---|---|---|---|---|
| **B1** @mention palette 渲染一致性 | Panel: `interfaces/webchat/src/platform/wide/views/chat/{mention_palette.rs, composer/mod.rs:1027-1058}`；TUI: 无独立 @-mention overlay | Panel 已实装（首位 `@all` 固定行 + 队员行），2 测试；TUI **无 @mention overlay**（slash palette 是唯一命令面） | pi-CC/desktop-cc-gui 都有文件树 @；Panel 仅解析 `@<id>` 文本，无文件补全 | **Tier 1**：TUI 缺 @-mention 面（不算 L1 范围，新功能）→ 移到下轮 |
| **B2** 斜杠命令面板 fuzzy 匹配 + 高亮 | Panel: `composer/palette.rs::build_palette_entries`（substring 匹配，无 fuzzy）12 测试；TUI: `widgets/command_palette.rs`（3 测试）+ `keys.rs::handle_palette_key`（5 测试）+ `command_tree.rs` | Panel: 命名空间树 + substring 匹配 + 描述拼接；TUI: 12 行 max + 选中高亮 + namespace_stack 面包屑；两边都没做 fuzzy / 模糊打分 | pi / codex 都有 fuzzy（subsequence / fuse）；这里只用 `contains` | **Tier 2**：fuzzy 是体验而非 bug，但与子项标题直接对应 |
| **B3** 多行编辑光标/选区在 Panel/TUI 行为差异 | Panel: `composer/mod.rs:171-185` `set_value + scroll_height` auto-grow；TUI: `widgets/input_area.rs::input_height` + `tui_textarea` | Panel 自动增高 + IME 候选窗**未保护** Enter（见 §3 #1）；TUI 行高钳制 `min/max`；两边都**无 `isComposing` IME 闸**（desktop-cc-gui 用 `isComposingRef + 100ms window` 防中文候选窗 Enter 误触发） | desktop-cc-gui 的 IME `isComposingRef` 与 `lastCompositionEndTimeRef` 在 100 ms 窗内一律判 composing | **Tier 1**：补 IME 闸（小） |
| **B4** attachment 拖拽/粘贴路径 | Panel: `composer/attachments.rs::read_file_list_into` 仅 `<input type="file">` 触发；TUI: 无 attach | ⚠️ 文件读取器 197 行 3 测试，**无 dragover / drop / 粘贴 handler**；`key=|(idx, f)| format!("{}:{}", idx, f.name)` 移除附件时 idx 漂移 → Leptos `<For>` 错误复用节点 | desktop-cc-gui `useComposerFileDrop`（depth counter 防 flicker）+ `useComposerImages`（粘贴图片 blob→attach）；Aleph 这条全无 | **Tier 1**：补 drag/drop 接收（小，但涉及键盘/鼠标多 surface 协调）或退一步先补粘贴（小） |
| **B5** voice 录音按钮状态机 | Panel: `composer/voice.rs` 612 行 `RecState::{Idle,Starting,Recording,Transcribing}` | 4 状态机 + native/浏览器双 backend；**0 测试**；在 `on_pointer_down` 里 `if state != Idle { return }` 但 **不清理遗留 timer**；`finish` 在 `Starting` 调用会触发 `recorder.stop()` on `None`（静默失败） | desktop-cc-gui 同等 audio capture 复杂度，测试覆盖更密 | **Tier 1**：补状态机测试（必先于修复，否则无从证伪 ≥0） |
| **B6** approval 卡片按键反馈（输入框 + approval 串行） | TUI: `tui/approval.rs::handle_approval_key`（7 测试）+ `widgets/dialog.rs::render_approval`；Panel: `components/approval_card.rs` 257 行 **0 测试** | TUI 数字键 + ↑↓ + Enter 三种路径齐；红边框与 AskUser overlay 区分；**无 Deny-with-reason 输入**（FEATURE_LOCATOR 显式延后）；Panel card 无组件级测试 | pi-ask-user：单/多选 + freeform + allowComment + overlay toggle + contextExpanded | **Tier 1**：Panel approval card 加组件测试（中）；TUI Deny-with-reason（小） |
| **B7** ask_user / clarification 卡片生命周期 | Core: `clarification/ask.rs::ask` + `RetireOnAbandon` RAII；Panel: `composer/mod.rs::answer_pending_ask`（stale 探测回退到普通 send）；TUI: `keys.rs::handle_dialog_key` + `widgets/dialog.rs::render_dialog` | 三面一致：retire-on-abandon RAII ✓、secret 分区 ✓、多问题游标 ✓、resolve by id + 答案游标 ✓；**clarification/ask.rs 743 行只有 10 个测试**（9 tokio + 1 sync），密度 < 1/74 行 | pi-ask-user：herdr:blocked 事件 + ask:answered/cancelled + details payload + session state 重建 | **Tier 1**：补 ask.rs 测试密度（中，target ≥20） |
| **B8** freeform / structured 切换 + 上下文折叠 | Panel: `chat/interface/has_quick_pick()` 切 menu/typing；TUI: `handle_dialog_key::KeyCode::Tab` 切 typing 态；Core: `ask_user.rs` `multi_select + secret` | 三面都支持 freeform/typing 切换；**无 `singleSelectLayout` (auto/list)**、**无 `contextExpanded`**、**无 `allowComment`**、**无 `overlayToggleKey` / `commentToggleKey`** | pi-ask-user：以上 4 项全有，且 `ctrl+e` 可热切 | **Tier 3**：4 个均非 bug，按"不做新功能"原则**不在本轮**；自然落下一轮（参考 pi-ask-user 补 UX 模式轮） |

---

## 3. 潜在 bug 热点

| # | 文件:行 | 代码片段 | 现象 | 修复方向 | Tier / 估计行数 |
|---|---|---|---|---|---|
| 1 | `composer/mod.rs:1078-1118` `on:keydown` Enter 分支 | ```if ev.key() == "Enter" && !ev.shift_key() { ... ev.prevent_default(); if answer_pending_ask() { return; } ... send_message() / enqueue_message(); }``` | **中文 IME 候选窗未护栏**：候选提交时 `isComposing` 为 false（候选窗关闭后 keydown 才到），会误触发 send；desktop-cc-gui 用 100 ms "recently settled" window 防此问题。FEATURE_LOCATOR 2244 行提到 "`is_composing()` 必须判——中文候选窗开着时草稿读起来是空的"，但实现里只在 Alt+↑ 路径走 `should_recall_on_bare_arrow_up`，Enter 路径完全没看 IME。 | 加 `compositionstart/compositionend` listener + 100 ms 闸，与 desktop-cc-gui 同形 | **Tier 1 / 小（<20 行）** |
| 2 | `composer/attachments.rs:144` `<For key=|(idx, f)| format!("{}:{}", idx, f.name)>` | 移除附件后 idx 重排，下一帧 `<For>` 复用错位节点 | 用户点中间一个 chip 的 ✕，Leptos 拿原 key 匹配，会让错误 chip 闪一下消失 | key 改为稳定 hash（name + size）或用 idx-only 然后强制 `<For each>` 重建 | **Tier 1 / 小（<10 行）** |
| 3 | `composer/voice.rs:455-475` `on_pointer_down` + `on_pointer_up` 状态转换 | `on_pointer_down` 早返回 `if state != Idle { return; }`，**遗留 `press_timer` 未清**；`finish` 在 state==Starting 时调用会执行 `recorder.stop()` on `None`（静默失败） | (a) 录音中按 Stop 按钮再释放会触发两次 ghost `finish`；(b) 状态机无测试覆盖，无法证伪 | 先补状态机测试（必前置），再补 timer 清理；`finish` 增加 `if state == Starting { return; }` 闸 | **Tier 1 / 中（先测试 30 行，修复 15 行）** |
| 4 | `clarification/ask.rs` 模块 | 743 行，只有 10 个测试（9 tokio + 1 sync），平均 74 行/测试；覆盖关键路径——channel send vs bus fallback、secret 分区、abandon drop、unattended refusal——已写但**未覆盖**：多 question 顺序 answer（仅 partition 写了两 question 的 partition 测试，无 4-question 全答题）、超时 cleanup_expired 的级联 reap、`withheld_secret` 在 Panel turn 的形状 | 风险：之后任何重构都缺少「失败时这一问答案会不会丢」的快速反馈；现有 7 个 `#[tokio::test]` 路径未覆盖"4 问题中 2 答 1 超时 1 撤回"的复合场景 | 补 § 6 列出的测试矩阵（小 5–10 测试，每个 20–40 行） | **Tier 1 / 中（5–10 测试，约 200–400 行）** |
| 5 | `approval_card.rs` Panel 组件 | 257 行 **0 测试**（仅模块级 `resolve()` helper 无 #[test]） | 无任何对渲染倒计时、按钮事件、deny-with-reason 输入、reason 持久化的覆盖 | 补组件级 snapshot 测试 + `resolve()` 单元测试（spawn_local 需要 mock ExecApprovalApi） | **Tier 1 / 中（100–200 行）** |
| 6 | `dialog.rs::render_approval` 选项数 >3 时 | `let option_count = u16::try_from(approval.decisions.len()).unwrap_or(3);` 然后 `let height = (option_count.saturating_add(7)).min(area.height);` | `unwrap_or(3)` 在"超过 u16::MAX"时让 layout 高度按 3 算 — 实际 `Vec<(&str, &str)>` 不会爆 u16，但 `min(area.height)` 在终端太矮时**裁剪选项而非滚动** | 屏保 sidebar `max-h-[200px] overflow-y-auto` 已存在，但 TUI 这里没滚动指示（用户可能不知道还有选项） | **Tier 3 / 小**（仅在决策列表常 >4 时显形） |
| 7 | `command_palette.rs::render_command_palette` widget | 159 行，3 测试；测试只覆盖 `filtered.is_empty()`、`MAX_VISIBLE_ITEMS`、`namespace_stack`，**未覆盖**实际渲染（缺 TestBackend） | 视觉回归无防：边框、focus 状态、选中高亮、空 namespace 标题全无测试 | 补 ratatui TestBackend snapshot 测试（参考 `widgets/input_area.rs` 的 7 个测试） | **Tier 2 / 小（30–60 行）** |
| 8 | `composer/mention_palette.rs:159-185` `update_mention_palette` | 过滤后 `selected_index.set(0)` 每次 `on:input` 都重置 | 用户 `@al` 选中第二项（"alex"）继续打字 → `selected_index` 跳回 0，体验不一致（codex 行为是保持） | 选项：仅在 mention 起始/终止时重置 | **Tier 3 / 极小**（<5 行） |
| 9 | `composer/mod.rs::send_message` 团队分支队列入参 | 团队分支 `if let Some(team_id) = ...` 拒绝带附件时 `chat.set_send_error` 但**未恢复已 typed 的 attachment**（已 typed 但塞进 `text` 之前已 `attachments.set(Vec::new())`） | 团队聊天粘贴附件 → Enter → 看到红色 banner，**但附件 chip 已清空**（与单聊 "failed_send_restores_the_tray" 测试钉住的形状相反） | 团队分支也走 `seed_draft(files)` 恢复 | **Tier 1 / 小（<20 行）** |
| 10 | `TUI keys.rs::handle_input_key` 上箭头 | `if lines.len() > 1 { textarea.input(...); return; }` 然后**历史浏览**逻辑 | `lines.len() > 1` 才把 ↑ 给光标；空 textarea 内多行粘贴后按 ↑ 正常浏览历史 — 但**光标在中间行**时按 ↑ 是移动光标而非浏览历史 | 边界合理但与 B3 直接相关：textarea 第一行 vs 第二行的行为差异不直观 | **Tier 3 / 不修**（属设计） |

---

## 4. pi-ask-user 模式差距

| pi-ask-user 模式 | Aleph 当前实现 | 差距 | L1 范围？ | Tier |
|---|---|---|---|---|
| split-pane（auto 详情预览 + list 单列持久化） | **无**：`dialog.rs::render_dialog` 一律竖排 `option_count` 行 | auto layout 缺失 → 小终端 description 被裁 | 不算 L1（新增 UI 形状） | Tier 3 |
| overlay toggle（`alt+o` 临时隐藏） | **无** | 同一 | 不算 L1 | Tier 3 |
| multi-select | ✅ 已实现（`ask_user.rs::multi_select` + `dialog.rs::multi_select` 分支） | 已对齐 | — | — |
| freeform | ✅ 已实现（`allowFreeform` 同形 `DialogState.typing` + Tab 切） | 已对齐 | — | — |
| persistent toggle（注释/上下文开关） | **无** | 同一 | 不算 L1 | Tier 3 |
| keybinding：`alt+o` overlay toggle | **无**（Esc 走全局，dialog 不响应 Esc，approval 不响应 Esc） | 与"AskUser overlay 不可逃"语义对齐，但缺 toggle 切换 | 不算 L1（新增） | Tier 3 |
| keybinding：`ctrl+g` 评论开关 | **无** | 同上 | 同上 | Tier 3 |
| keybinding：`ctrl+e` 上下文展开 | **无** | 同上 | 同上 | Tier 3 |
| `promptSnippet` / `promptGuidelines` 系统 prompt 集成 | `clarification/ask.rs::ask()` 已通过 `turn.run_id` 关联（FEATURE_LOCATOR §5.3 第六轮），但**无与 pi-ask-user 同形的 promptGuidelines 注入** | 不算 L1 | Tier 3 |
| `herdr:blocked` lifecycle | **无显式 herdr 事件**（Aleph 走 `ToolCallParked` + `ClarificationEnded` wire frame） | 同源不同形，已超参考实现 | — | — |
| `details` payload（session state 重建） | ✅ `AskUserOutput.answers + withheld + status` | 已超参考实现 | — | — |
| Escape fallback | ✅ TUI/global handler 显式吞 Esc（`dialog is_some` → Action::None），Panel: `dialog.rs::render_dialog` 是唯一出口 | 已对齐 | — | — |
| `singleSelectLayout` (auto / list) | **无** | 不算 L1 | Tier 3 |
| `contextExpanded` | **无** | 不算 L1 | Tier 3 |
| 强制决策握手 skill (`ask-user` skill) | **无对应 skill**（Aleph 走 model 自学 + system prompt） | 不算 L1（skill 体系在另一赛道） | Tier 3 |
| `displayMode` (overlay/inline) | **无** | 不算 L1 | Tier 3 |
| `comment` 自由评论 | **无** | 不算 L1 | Tier 3 |

**结论**：pi-ask-user 大部分模式是"新功能"而非 bug。L1 范围已覆盖核心生命周期（已超参考）；其余 7 项均属 Tier 3（下一轮）。

---

## 5. pi-claude-code-tui 模式差距（仅输入/光标相关）

| pi-CC 模式 | Aleph TUI 当前 | Tier |
|---|---|---|
| `CodexStyleEditor` 半开圆角边框（顶部 ╭ 底部 ╯） | `widgets/input_area.rs::InputWidget::render` 用全宽单字符 rule（`\u{2500}` repeat） | Tier 3（视觉，与 spec Tier 3 性能/样式并行） |
| 闪烁金条光标（braille block） | `ta.set_cursor_style(Style::default().bg(Color::White).fg(Color::Black))` 静态反色 | Tier 3 |
| `❯` prompt 在 col 0 | `InputWidget` 第一行：`{CARET} ` 占 GUTTER=2 列；多行时**不重复**（`caret_area` 只高度 1） | ✅ 与 pi-CC 同形（多行无重复 mark） |
| 滚动提示（`↑↓ N more`） | TUI 缺 | Tier 3 |
| Shift+Tab 切 Plan/Auto Mode | TUI 无对应（Plan mode 在另一 surface） | 不算 L1 |
| autocomplete rows 弹在 box 上方（CC 习惯） | TUI 无 overlay | Tier 3 |
| IME composition 保护 | TUI 走 `tui_textarea`，库本身有 compositionend 钩子但 Aleph 不接 | TUI 终端通常不直接收 IME 信号（依赖 kitty/iTerm 转义），不显形 |

---

## 7. desktop-cc-gui 模式差距（仅 composer 相关）

| desktop-cc-gui 模式 | Aleph Panel 当前 | Tier |
|---|---|---|
| IME composition guard (`isComposingRef` + 100 ms window) | **无**（§ 3 #1） | **Tier 1** |
| Drag OS files → tauri-vim event listener (whole window, hit-test zone) | **无**：只有 `<input type="file">` click（§ 3 #4） | **Tier 1** |
| Paste images → `onPasteImages(files: File[])` → attach | **无**：仅 `<textarea>` 默认 paste 文本 | **Tier 1** |
| Ghost-text completion (`data-completion-suffix` + Tab 接受) | 无（FEATURE_LOCATOR 2186 行有 queued ghost 但实现不同） | Tier 3 |
| File tags rendering (highlight `@xxx` mentions in composer DOM) | 无（仅 `@id` 文本 strip） | Tier 3 |
| Top-edge drag resize (handle fixes field at explicit height) | TUI: `input_height()` 行高钳制；Panel: `auto-grow` + ResizeObserver | 已对齐 |
| AddMenu slot (template AddMenu) | Panel: 附件 + voice + project + model + mode + tier + dial + gauge | 已超越 |
| `usePromptHistoryNav` (↑↓ recall sent prompts) | Panel: `chat.recall_latest_queued` + `seed_draft` 仅召回**未发送**的 queued；无 sent-history 滚动 | Tier 2（小，存 `chat.send_history` 即可） |

---

## 8. 范围闸

### L1 范围：Y（仅 § 3 #1/#2/#3/#5/#9 + § 5 的 IME 闸 + § 7 的 IME 闸）

### 不在 spec 范围（移到下轮）：
- B1 TUI @-mention overlay（新功能）
- B2 fuzzy 匹配（UX 增强，非 bug）
- B8 `singleSelectLayout` / `contextExpanded` / `allowComment` / `overlayToggleKey` / `commentToggleKey`（新功能，FEATURE_LOCATOR §5.3 第六轮已显式排除）
- § 4 pi-ask-user 7 项模式（Tier 3 UX 扩展）
- § 5 pi-CC 6 项（TUI 视觉打磨，Tier 3）
- § 7 desktop-cc-gui 4 项（Tier 3 新功能）
- drag-drop 文件 / 粘贴图片（虽算 Tier 1 但涉及 Tauri `dragDropEnabled` 与 webview 跨 surface，**L1 仅限 Panel 粘贴图片**；OS 拖放见下轮）

### Tier 候选汇总

**Tier-1（建议本轮做）**：
1. § 3 #1 — Panel composer IME 闸（小，<20 行 + 2 测试）
2. § 3 #2 — attachment `For` key 稳定性（小，<10 行 + 1 测试）
3. § 3 #3 — voice 状态机先补测试后修（中，~45 行）
4. § 3 #4 — `clarification/ask.rs` 测试密度提升（中，~200–400 行）
5. § 3 #5 — `approval_card.rs` Panel 组件测试（中，~100–200 行）
6. § 3 #9 — team-chat 附件拒绝时也恢复 tray（小，<20 行）
7. § 7 — Panel 粘贴图片 attach（小，<30 行，跨 `attachments.rs::read_file_list_into` 复用 + `on:paste` handler）

**Tier-2（可选）**：
- § 3 #7 — `command_palette.rs` 渲染 snapshot 测试（小）
- § 7 — `usePromptHistoryNav` 等价物（小）

**Tier-3（下轮）**：
- B1 TUI @-mention
- B2 fuzzy
- B8 全套 pi-ask-user UX 扩展
- pi-CC 视觉打磨
- 大部分 desktop-cc-gui 模式

### "先补测试再修"清单
- voice 状态机（#3）—— 0 测试，无法证伪
- approval_card Panel 组件（#5）—— 0 测试，组件级风险
- clarification/ask.rs（#4）—— 密度 < 1/74 行

---

## 总结

- **Tier-1 候选数**：7 项（含 3 项"先补测试再修"）
- **Tier-2 候选数**：2 项
- **Tier-3 候选数**：8+ 项（已超出 spec L1 范围）
- **薄覆盖区警告**：`voice.rs`（612 行，0 测试）、`approval_card.rs`（257 行，0 测试）、`clarification/ask.rs`（743 行，10 测试，密度低）—— 这三处任何重构都缺快速失败护栏
- **L1 共识**：本轮可做 7 项 Tier-1（其中 3 项必须先补测试），总修改量预估 < 800 行
- **track-b-findings.md 路径**：`docs/superpowers/plans/Phase1-audit/track-b-findings.md`
- **sha**：HEAD `5e1b29062`（worktree HEAD，基于 `0927461ba`）