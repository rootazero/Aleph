# Track D Findings — 共享契约

> **Spec 关联**：`docs/superpowers/specs/2026-09-21-panel-tui-polish-design.md` §3 D（D1–D4）
> **时间**：2026-09-21（worktree panel-tui-polish @ 5585098）
> **关联 baseline**：`0927461ba`（spec §1）
> **L1 范围**：仅修 bug + 补连线 + 补测试；**不抽新公共面**（spec §7）

---

## 1. FEATURE_LOCATOR 已知未完成项（共享契约相关）

| 口语关键词 | Anchor | 状态 | 与 D1–D4 对应 |
|------------|--------|------|---------------|
| Composer Session Dials (think / memory / pin) | `views/chat/dial_picker.rs` + `api/sessions.rs` (`SessionRowKnobs`) + `shared/ui_logic/src/state/composer_dials.rs::SessionKnobs` | ✅ (§5.23b · round-2 wire 面 2026-09-02) | **D1** — Panel 已用 `aleph_protocol::SessionListRow`，自己的 `SessionRow` 已删 |
| Durable Model Pin (4th Twin) | `gateway/session_model_pin.rs` + `session_model_handle::MODEL_PIN_SESSION_KEY` | ✅ (§5.23, 2026-08-11) | **D1** — wire & server 已通；**TUI 显示层未消费**（见 §3.a） |
| Per-Session Memory Mode (5th Twin) | `memory/session_memory_mode.rs` + `harness_bridge/prompt_build.rs` | ✅ (§5.23) | **D1** — Panel 与 TUI 均消费 session_memory_mode；一致 |
| Atomic Session Metadata Write | `gateway/session_store/file_backend/mod.rs::write_metadata` (`utils::atomic_write`) | ✅ (§5.23b) | **D4** — `atomic_write_file` 走 temp+fsync+rename；`lock_metadata` 持锁（详见 §6） |
| Frame-to-Conversation Routing | `platform/wide/views/chat/events.rs::resolve_target`（三步）+ `state/sessions.rs::set_session_key` | ✅ (§6.9, 2026-08-10) | **D4** — 三步仲裁已固化为单测；Panel 走 string-dispatch，TUI 走 typed-enum |

> **关键观察**：5 条 ✅ 项的「server 落点」均无缺口；缺口集中在「Panel/TUI 客户端对同一形状的消费一致性」（§3 / §4）。

---

## 2. Spec §3 D 子项覆盖矩阵

| 子项 | Aleph 锚点 | 现状 | Tier 候选 |
|------|-----------|------|-----------|
| **D1** SessionKnobs / ComposerSessionDials 两端共用 | `shared/ui_logic::state::composer_dials::SessionKnobs`（5 字段：`exec_tier`/`mode`/`think_level`/`memory_mode`/`model_pin`）+ `SendDials` + `session_dials_for_send`；Panel `api::sessions::SessionRowKnobs.knobs()` 返回 5 字段；TUI `app/mod.rs::SessionKnobs<'a>`（**4 字段，无 `model_pin`**） | **不一致**：Panel 用 shared 5 字段；TUI 自己定义 4 字段，**丢掉了 `model_pin` 的显示** | **Tier-1**（行为差异，用户可见） |
| **D2** 事件投影（`StreamEvent` → Panel/TUI 内部类型） | TUI：`interfaces/tui/src/tui/app/events.rs`（typed-enum，**26** 处 `StreamEvent::*` 穷尽匹配）；Panel：`platform/wide/views/chat/events.rs`（**string-dispatch**，`event_type == "..."`，13 处） | **形状不同** + **事件覆盖不同**：TUI 处理 `ReasoningBlock` / `UncertaintySignal`，**Panel 完全未消费**（grep 0 hit） | **Tier-2**（事件落地差异，需确认是否真为事件需要） |
| **D3** keymap 共享（hotkey vs TUI keys 对齐） | Panel `state/hotkey.rs`（⌘K palette / ⌘⇧V voice / Esc / f doctor）；TUI `tui/keys.rs`（Esc overlay / Ctrl+C cascade cancel / Ctrl+D quit / Tab focus / Enter 与 Shift+Enter 区分） | **平台语义天然不同**，但 Esc 关闭 overlay、Ctrl+C 取消运行的「基本一致」 | 无候选（结构性差异，不属于 L1 bug） |
| **D4** atomic session metadata write | `utils::atomic_write::atomic_write_bytes`（tempfile + fsync + rename，POSIX 单 FS 原子）+ `meta::write` 唯一调用 + `MetaGuard` 持锁 | **已 atomic**；race window：4 个「operator-initiated」重写（`truncate_messages` / `retire_from` / `restore_checkpoint` / `branch_from_checkpoint`）**不持锁**（代码注释自承「bounded gap」） | **Tier-3**（scope 外） |

---

## 3. SessionKnobs / SessionRowKnobs / SessionListRow / SessionRowKnobs 类型一致性

### 3.a 定义位置

| 类型 | 位置 | 字段数 | 字段名 |
|------|------|--------|--------|
| `aleph_protocol::SessionListRow`（wire） | `shared/protocol/src/sessions.rs:30` | 22 字段 | 含 `exec_tier` / `mode` / `think_level` / `memory_mode` / `model_pin` |
| `aleph_protocol::SessionSnapshot`（attach 响应） | `shared/protocol/src/session_thread.rs:259` | 17 字段 | 含 `exec_tier` / `mode` / `think_level` / `memory_mode` / `model_pin` / `model_pin_provider` |
| `shared_ui_logic::state::SessionKnobs`（surface 值） | `shared/ui_logic/src/state/composer_dials.rs:31` | **5 字段** | `exec_tier` / `mode` / `think_level` / `memory_mode` / `model_pin` |
| `shared_ui_logic::state::SendDials`（wire 出） | 同上 :75 | 4 字段 | `exec_tier` / `mode` / `thinking` / `memory` |
| Panel `SessionRowKnobs for SessionListRow` | `interfaces/webchat/src/api/sessions.rs:143` | 5 字段 | 5/5 全映射 |
| Panel `ChatState::session_knobs` | `interfaces/webchat/src/platform/wide/views/chat/state/mod.rs:1875` | 5 字段 | 5/5 全映射 |
| **TUI `app::SessionKnobs<'a>`** | **`interfaces/tui/src/tui/app/mod.rs:648`** | **4 字段** | **`mode` / `exec_tier` / `think_level` / `memory_mode`**；**`model_pin` 缺失** |
| TUI `AppState::session_knobs` | `interfaces/tui/src/tui/app/mod.rs:1966` | 4 字段 | 返回上 4 字段；未消费 `session_snapshot.model_pin` |
| TUI `slash::SessionKnob`（enum） | `interfaces/tui/src/tui/slash.rs:120` | 4 变体 | `ExecTier` / `Mode` / `Think` / `Memory`；**无 `ModelPin` 变体**（注释明示「pin 走 `select_model`，不是写会话 bag」） |

### 3.b 关键不对称

- **Panel 端**：Panel 已收敛到 shared 5 字段（`SessionRowKnobs::knobs()` + `ChatState::session_knobs()` 都包含 `model_pin`）。
- **TUI 端**：`app::SessionKnobs<'a>` 是本地 4 字段结构体；`session_knobs()` 把 `SessionSnapshot.model_pin` 直接丢掉。**TUI 状态条不渲染 pin**。`app/mod.rs:631-643` 注释明确「status bar enumerates `slash::SessionKnob::ALL`」（4 个），`record_local_knob` 只匹配 4 个；新增 dial 需要改 5 处，但**这就是结构本身，不是「保证正确性」**。
- 与 `SessionSnapshot.effective_model()`（`session_thread.rs`）比较：TUI 的 `app::state.model_name` 走另一条路，不通过 `session_knobs()`，所以单独工作；但「TUI 不知道有 pin」≠「无 pin」——如果未来要给 TUI 加 pin 显示，需要先把 `SessionKnobs` 提升到 shared（违反 L1）或者复制 5 字段到 TUI 本地（YAGNI 重影）。

### 3.c 「source of truth」边界

- **wire source of truth**：`aleph_protocol::SessionListRow`（服务端构造，客户端零手写）。
- **surface source of truth**：`shared_ui_logic::state::SessionKnobs`（Panel 用，**TUI 不用**）。
- **session_dials_for_send 的 5→4 缩减**正确：`SendDials` 故意不带 `model_pin`（注释说：「pin 不是 send 字段，select_model 是唯一写者」），已由测试 `the_model_pin_never_rides_a_send` 钉住。
- `session_dials_for_send` 的「**tier 每次带 / 其他四个只有首条带**」规则四个发送面都遵守（`api::chat::tests::every_send_path_resolves_the_dials_through_the_shared_rule`，变异证过 RED）。

---

## 4. StreamEvent 投影

### 4.a 形状对比

| 端 | 投影机制 | 路径 | 字段穷尽保证 |
|----|----------|------|--------------|
| **TUI** | typed-enum 模式匹配 | `interfaces/tui/src/tui/app/events.rs` 26 处 `StreamEvent::*` | ✅ 编译时穷尽；`run_id()` 在 `shared/protocol/src/events.rs:649` 也是穷尽 match |
| **Panel** | string-dispatch（`event_type == "..."`） | `platform/wide/views/chat/events.rs` 13 个字符串 | ❌ 任何新增事件都是「静默 no-op」；`subscribe_run_events` 顶层 `if !event.topic.starts_with("run.") { return; }` 后只剩字符串分支 |

### 4.b 事件覆盖差异

| StreamEvent 变体 | wire `type` 字符串 | Panel 是否处理 | TUI 是否处理 |
|------------------|------------------|----------------|--------------|
| `RunAccepted` | `run_accepted` | ✅ | ✅ |
| `RunQueued` | `run_queued` | ✅ | ✅ |
| `RunComplete` | `run_complete` | ✅ | ✅ |
| `RunError` | `run_error` | ✅ | ✅ |
| `ResponseChunk` | `response_chunk` | ✅ | ✅ |
| `Reasoning` | `reasoning` | ✅（仅 `run_id.is_empty()` 路径） | ✅ |
| `ToolStart` / `ToolUpdate` / `ToolEnd` | `tool_start` / `tool_update` / `tool_end` | ✅ `tool_start` + `tool_end`；**`tool_update` 无字符串分支**（grep 仅在 resolve_target 测试里） | ✅ |
| `AgentTrace` | `agent_trace` | ✅ 通过 `apply_trace_event`（共享投影） | ✅ |
| `AskUser` | `ask_user` | ✅ | ✅ |
| `ClarificationEnded` | `clarification_ended` | ✅ | ✅ |
| `SessionUserMessage` | `session_user_message` | ✅ | ✅ |
| `ModelResolved` | `model_resolved` | ✅ | ✅ |
| `RunRetrying` | `run_retrying` | ✅ | ✅ |
| `ContextGauge` | `context_gauge` | ✅ | ✅ |
| **`ReasoningBlock`** | **`reasoning_block`** | ❌ **Panel 0 hit** | ✅ |
| **`UncertaintySignal`** | **`uncertainty_signal`** | ❌ **Panel 0 hit** | ✅ |

### 4.c 现象

- `ReasoningBlock` 与 `UncertaintySignal` 这两个事件由 `aleph_protocol::StreamEvent` 定义、带 `seq` 与结构化字段（`ConfidenceLevel` / `UncertaintyAction`），但在 Panel 端被静默吞掉。Panel 用户看不到「结构化推理 step 类型」徽标、不显示不确定性提示。
- 这不是新 bug：FEATURE_LOCATOR §5.23b round-2 那一轮明确把 `Panel 自己的 SessionRow` 删了，但**事件投影端没做对账**。
- TUI 处理 `ReasoningBlock`（`tui/app/events.rs:675`）的逻辑是「仅当不是 `current_run_uses_agent_trace` 才 append」——避免与 `agent_trace.text_emitted` 重复；Panel 已经走 `agent_trace` 路径，所以即便加了 `reasoning_block` 也需要同样护栏。

---

## 5. Keymap 共享

### 5.a Panel hotkey 清单（`state/hotkey.rs`）

| 触发 | 行为 | 备注 |
|------|------|------|
| ⌘K / Ctrl+K | toggle palette | 全局 |
| ⌘⇧V / Ctrl+Alt+V | toggle voice | 平台分支 |
| Esc + voice open | close voice overlay + prevent_default | 抢占 |
| Esc + palette open | close palette（不 prevent_default） | 让其他 Esc 处理器继续 |
| bare `f`（无修饰、焦点不在 editable） | doctor + LLM repair（G1） | 强护栏 |

### 5.b TUI keys 清单（`tui/keys.rs`）

| 触发 | 行为 | 备注 |
|------|------|------|
| Esc | 关闭 CommandPalette / 退出 Agents detail / BTW 模式翻页 | 不关 AskUser dialog（注释：dialog 是 server-side parked run，关闭会孤立 run） |
| Ctrl+C | cancel run → 清空输入 → 第二次按 → quit（"Press Ctrl+C again to quit."） | smart cascade |
| Ctrl+D | quit 立即 | — |
| Enter | send / Shift+Enter newline / Ctrl+Enter newline / `\` 续行 newline | portable newline（Windows Terminal/WSL/SSH） |
| Ctrl+J | newline | portable newline |
| Tab | focus chat ↔ input（在 dialog 是 menu ↔ typing） | 多义复用 |
| j/k | chat 焦点滚动 | vim-style |

### 5.c 对齐点

| 语义 | Panel | TUI | 一致？ |
|------|-------|-----|--------|
| Esc 关闭 overlay | ✅ | ✅ | ✅（语义一致：都不关 AskUser） |
| Ctrl+C 取消运行 | ✅（按钮） | ✅（hotkey，smart cascade） | ✅ |
| 提交消息 | Enter | Enter（裸） | ✅（语义一致，modifier 含义略有不同） |
| 换行 | Shift+Enter | Shift+Enter / Ctrl+Enter / `\` / Ctrl+J | ⚠️ 多重 fallback，理由是 terminal 兼容 |
| 关闭命令面板 | Esc | Esc | ✅ |
| 调出命令面板 | ⌘K | `/`（输入框首字符） | ⚠️ 触发键不同，UI 形态不同（modal vs inline）——不算 bug |

### 5.d 结论

keymap 共享**没找到 bug**。两端共享的是「意图」而不是「字面按键」（R8「不做意图分类」也只对 LLM 工具路由说），Esc 行为、Enter 语义、取消 run 的 hotkey 路径都覆盖到了。这一块 L1 不需要动。

---

## 6. Atomic write

### 6.a 路径与实现

- `atomic_write_bytes`（`src/utils/atomic_write.rs:34`）：
  1. `tempfile::Builder::new().prefix(".aleph_atomic_").suffix(".tmp").tempfile_in(parent)`（同目录 staging）
  2. `tokio::fs::write(&tmp_path, content)`
  3. `fs::OpenOptions::new().write(true).open(&tmp_path)` + `file.sync_all()`
  4. `set_permissions` 继承（仅 `metadata` 存在时）
  5. `fs::rename(&tmp_path, path)`（POSIX rename 原子）
- `meta::write`（`src/gateway/session_store/file_backend/meta.rs:216`）：上述函数唯一调用方，针对 `metadata.json`。
- `MetaGuard::commit`（同上 :161）：`write(&self.path, &meta).await?`——所有 `sessions.patch` 类写入必经此路径。

### 6.b 锁覆盖

- ✅ `lock_metadata` 持锁路径：`append_message`（:865 + :912 + :1033 + :1038 持锁至 commit）、`stamp_assistant_metadata_in_range`（:1488 + :1548）、`write_metadata` 经 `MetaGuard::commit`。
- ❌ **不持锁路径**（`mod.rs:315-320` 注释自承）：
  - `truncate_messages`（operator-initiated）
  - `retire_from`（operator-initiated）
  - `restore_checkpoint`（operator-initiated）
  - `branch_from_checkpoint`（operator-initiated）

  这四条**重写整个 transcript.jsonl**（也是用 `atomic_write_file`），「原子保证 survivor 完整」，但**不持锁意味着两个并发操作中 loser 的写入被丢弃**——典型的「last-writer-wins」，对 operator 重写操作可接受（注释：「bounded gap」），但与 `append_message` 并发会丢最后一条消息（极小概率，但 spec 已经点名）。

### 6.c 结论

**真 atomic**（POSIX rename 语义保证），race window 仅限上面 4 个 operator-initiated 重写；属 spec §6 风险表中「跨子系统改动」类，本轮 L1 不动。**无 Tier 候选**。

---

## 7. 潜在 bug 热点

| # | 文件:行 | 代码片段 | 现象 | 修复方向 | Tier | 行数估 |
|---|---------|----------|------|----------|------|--------|
| **D1.1** | `interfaces/tui/src/tui/app/mod.rs:648-653` | `pub struct SessionKnobs<'a> { mode, exec_tier, think_level, memory_mode }`（**4 字段**） | TUI 状态条**不显示 model_pin**。`SessionSnapshot` 带 `model_pin` 与 `model_pin_provider`，但 `session_knobs()` 丢弃。注释明示 status bar 只 enumerates 4 个 dial | 在本地 struct 加 `model_pin: Option<&'a str>`，并在 `session_knobs()` 里 `snap.and_then(\|s\| s.model_pin.as_deref())`。**不动 shared 类型**（L1 不抽新公共面） | **Tier-1**（用户可见：pin 完模型后 TUI 不显示；FEATURE_LOCATOR §5.23 「panel 看不到模型 pin 了 pill 还写 Default」的对偶，TUI 是「在面板中不看」的具体化） | **小** <20 |
| **D2.1** | `interfaces/webchat/src/platform/wide/views/chat/events.rs:1148-1270`（约 12 处 `event_type == "..."` 字符串分支） | Panel string-dispatch 不处理 `reasoning_block` 与 `uncertainty_signal`（grep 0 hit）；TUI 已处理（`tui/app/events.rs:675, 684`） | Panel 用户看不到结构化推理 step 类型（`ConfidenceLevel`）和不确定性信号（`UncertaintyAction`）。`StreamEvent::ReasoningBlock` / `UncertaintySignal` 在 wire 上发出，被 Panel 静默吞 | 在 Panel 的 `subscribe_run_events` 加 `"reasoning_block"` 与 `"uncertainty_signal"` 两个分支，做最薄投影（Panel 不像 TUI 有「Reasoning 折叠」，建议先打一条轻量 system row 即可，参考 `chat.set_provider_retry` 的写法）；加单测覆盖 | **Tier-2**（行为差异，但用户感知弱：这两个事件源端未必每条都发；spec §3 A 提到「reasoning 折叠」但没说徽标） | **中** 20-100 |
| **D2.2** | `interfaces/webchat/src/platform/wide/views/chat/events.rs` | Panel `event_type` 字符串字面量散落 ~13 处，无集中表 | 任何新增 `StreamEvent` 变体在 Panel 端是**编译能过、运行时 no-op**；TUI 端是编译错误（typed-enum 穷尽） | 不抽公共表（违反 L1），但可加一个 `#[cfg(test)]` 静态扫描断言「每个 wire 类型字符串都被字符串 switch 覆盖」（参考 `api::chat::tests::every_send_path_resolves_the_dials_through_the_shared_rule` 的变异证过 RED 模式） | **Tier-2**（护栏测试） | **中** 20-100 |
| **D4.1** | `src/gateway/session_store/file_backend/mod.rs:315-320`（注释） | `truncate_messages` / `retire_from` / `restore_checkpoint` / `branch_from_checkpoint` 不持 `lock_metadata` | 与 `append_message` 并发时 last-writer-wins，loser 写入丢弃；概率低但 spec 已点名 | 给这 4 个 operator-initiated 重写也加 `lock_metadata` 持锁（修改简单） | **Tier-3**（scope 外，operator 路径，且注释明示「bounded gap，scope 之外」） | **小** <20（注释 + 4 处锁获取），但**不属于 L1** |
| **D3.1** | — | — | 未发现 keymap bug。Esc / Ctrl+C / Enter 语义两端一致 | — | — | — |

---

## 8. 范围闸

### L1 范围：Y

- 仅修 bug + 补连线 + 补测试；不抽新类型、不动 wire schema。
- **不抽新公共面**：spec §7 明示，且 §2 原则「连线优先」。

### Tier 候选汇总

| Tier | 数量 | 项目 |
|------|------|------|
| **Tier-1**（用户立刻可见） | **1** | **D1.1** TUI SessionKnobs 缺 `model_pin` 显示（<20 行） |
| **Tier-2**（行为差异 / 加固测试） | **2** | **D2.1** Panel 不消费 `ReasoningBlock` / `UncertaintySignal`；**D2.2** Panel string-dispatch 缺静态覆盖测试 |
| **Tier-3**（scope 外 / 性能） | **1** | **D4.1** 4 个 operator-initiated transcript 重写不持锁（注释自承「bounded gap」，spec §7 不动 wire / 不做新功能；这条更近「连线补完」，可争 Tier-2） |

### Phase 1 建议交付顺序

1. **D1.1**（Tier-1，最直接、最小）
2. **D2.2**（Tier-2，先加护栏，**红**先验，再看是否需要 D2.1 真实投影）
3. **D2.1**（Tier-2，若端到端 Playwright/手测发现用户感知需要再上）
4. **D4.1** 留待用户裁定（注释明示「next step」）

### 范围之外（明确不做）

- ❌ 把 `TUI SessionKnobs` 提到 `shared_ui_logic` 加 `model_pin` 字段（违反 L1「不抽新公共面」；5 字段只有 Panel 用）
- ❌ 把 Panel 改成 typed-enum 投影（替换巨大而收益小，且 string-dispatch 注释里有设计理由）
- ❌ 动 `aleph_protocol::StreamEvent` / `SessionListRow` / `SessionSnapshot` 的字段（wire schema 不变）
- ❌ 改 `atomic_write` 实现（已正确）
- ❌ 改 keymap 字面按键（结构性差异，不属于 bug）

### 不做的事（State the Negative）

- **不**增新依赖
- **不**重写 D 赛道的现有文件结构
- **不**抽「Panel/TUI 共用 StreamEvent dispatcher」之类的公共面（违反 L1）
- **不**改 `parse_run_halt`（已验证「`run_complete` arm 不再投影 halt reason 时此函数变纯函数」测试存在）
- **不**改 `resolve_target` 的三步仲裁（已固化为单测）
- **不**重命名 `SessionRowKnobs` 扩展 trait（已说明放在 Panel 端而非 shared/protocol 的原因）