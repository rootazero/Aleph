# 工作流回显 × 工作区面板整合 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 Agent 多步执行（Think→Act 迭代）的回显在 WebChat Panel 左侧聊天按步分段成独立气泡、右侧工作区面板按迭代分组成步骤卡片，两侧以 `(run_id, iteration)` 为主键互点高亮，并原生兼容打字机/即时两种输出模式。

**Architecture:** 纯前端（`aleph-panel` crate）。以 `agent_trace`（两种 output_mode 下都即时发出、原生带 `iteration`）为左右两侧分步结构的唯一权威来源；`response_chunk` 退化为打字机模式的实时预览叠加，被 `agent_trace.text_emitted` 覆盖校正；即时模式忽略 `is_final` 的缓冲 dump。不碰 gateway / harness / 协议 / CLI。

**Tech Stack:** Rust + Leptos (WASM)，`RwSignal` 响应式状态，host-safe `#[test]` 单测（web_sys 以 `cfg(target_arch="wasm32")` 隔离）。

**关联 spec:** [docs/superpowers/specs/2026-06-05-workflow-echo-workspace-integration-design.md](../specs/2026-06-05-workflow-echo-workspace-integration-design.md)

**验证命令:**
- 单测：`cargo test -p aleph-panel <test_name>`
- 主机编译检查：`cargo check -p aleph-panel`
- WASM 打包：`just wasm`

---

## File Structure

| 文件 | 职责 | 改动 |
|------|------|------|
| `interfaces/webchat/src/views/chat/state.rs` | 聊天消息模型 + 气泡分步方法 | `ChatMessage.iteration` 字段；新增 `begin_step` / `set_step_text` |
| `interfaces/webchat/src/state/layout.rs` | 工作区状态 | `focused_step` / `current_iteration` 字段 + 方法 + `reset` 扩展 |
| `interfaces/webchat/src/views/chat/events.rs` | Gateway 事件 → 状态变更 | `turn_started` / `text_emitted` 分支 + `response_chunk` 规则改写 |
| `interfaces/webchat/src/components/workspace_panel.rs` | 右侧时间线渲染 | `timeline_groups` + `StepCard` 按迭代分组 + 高亮 |
| `interfaces/webchat/src/views/chat/messages.rs` | 左侧气泡渲染 | `run_id_from_message_id` 升级 + 迭代标签 + 气泡高亮 |
| `interfaces/webchat/src/components/chat_sidebar.rs` | 历史水合（仅补字段） | `ChatMessage` 字面量补 `iteration: None` |
| `interfaces/webchat/src/views/chat/timeline.rs` | 测试 helper（仅补字段） | 同上 |

---

## Task 1: ChatMessage 迭代字段 + 气泡分步方法

**Files:**
- Modify: `interfaces/webchat/src/views/chat/state.rs`
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:191`
- Modify: `interfaces/webchat/src/views/chat/timeline.rs:218`
- Modify: `interfaces/webchat/src/components/workspace_panel.rs:386`
- Test: `interfaces/webchat/src/views/chat/state.rs`（`#[cfg(test)] mod` 内）

- [ ] **Step 1: 给 `ChatMessage` 加 `iteration` 字段**

在 `state.rs` 的 `ChatMessage` 结构体（当前 `timestamp` 字段之后，约第 141 行）追加：

```rust
    /// Think→Act iteration this bubble belongs to, stamped from
    /// `agent_trace.turn_started`. `None` for user messages, the pre-turn
    /// placeholder, and legacy/hydrated history rows. Drives left-chat
    /// segmentation and the `(run_id, iteration)` cross-highlight key.
    #[serde(default)]
    pub iteration: Option<usize>,
```

- [ ] **Step 2: 给所有 `ChatMessage` 字面量补 `iteration`**

逐处在 `timestamp: ...` 行后加一行 `iteration: None,`：
- `state.rs` `push_user_message`（约 354 行 `timestamp: Some(...)` 之后）
- `state.rs` `start_assistant_message`（约 373 行之后）
- `state.rs` `finalize_intermediate`（约 410 行之后）
- `components/chat_sidebar.rs:191` 起的 `ChatMessage { ... }`（历史水合，补 `iteration: None,`）
- `views/chat/timeline.rs:218` 测试 helper `msg`（补 `iteration: None,`）
- `components/workspace_panel.rs:386` 测试 helper `msg_with_tools`（补 `iteration: None,`）

- [ ] **Step 3: 编译验证字面量已全部补齐**

Run: `cargo check -p aleph-panel`
Expected: PASS（无 "missing field `iteration`" 错误）

- [ ] **Step 4: 写失败测试 —— `begin_step` 复用空占位 / 分步新建**

在 `state.rs` 的 `#[cfg(test)] mod tests`（文件底部）内加入。若该文件尚无 tests 模块，新建：

```rust
#[cfg(test)]
mod step_tests {
    use super::*;
    use leptos::prelude::*;

    fn assistant_ids(chat: &ChatState) -> Vec<(String, Option<usize>, bool, bool)> {
        chat.messages.with(|m| {
            m.iter()
                .map(|x| (x.id.clone(), x.iteration, x.is_streaming, x.is_intermediate))
                .collect()
        })
    }

    #[test]
    fn begin_step_reuses_empty_placeholder() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);

        let rows = assistant_ids(&chat);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "assistant-r1");
        assert_eq!(rows[0].1, Some(1));
        assert!(rows[0].2, "reused placeholder still streaming");
    }

    #[test]
    fn begin_step_finalizes_nonempty_and_opens_new() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);
        chat.append_chunk("r1", "step one");
        chat.begin_step("r1", 2);

        let rows = assistant_ids(&chat);
        assert_eq!(rows.len(), 2);
        // First bubble finalized as intermediate, tagged iteration 1.
        assert!(rows[0].0.starts_with("intermediate-r1-"));
        assert_eq!(rows[0].1, Some(1));
        assert!(!rows[0].2 && rows[0].3, "finalized + intermediate");
        // Fresh streaming bubble tagged iteration 2.
        assert_eq!(rows[1].0, "assistant-r1");
        assert_eq!(rows[1].1, Some(2));
        assert!(rows[1].2 && !rows[1].3);
    }
}
```

- [ ] **Step 5: 运行测试，确认失败**

Run: `cargo test -p aleph-panel step_tests`
Expected: FAIL（`begin_step` 未定义）

- [ ] **Step 6: 实现 `begin_step`**

在 `state.rs` `ChatState` impl 内（紧邻 `finalize_intermediate` 之后）加入：

```rust
    /// Begin a new agent step (Think→Act iteration) for `run_id`.
    ///
    /// Driven by `agent_trace.turn_started`. If the current
    /// `assistant-{run_id}` bubble already carries text or tool calls, it is
    /// finalized as a standalone intermediate step and a fresh streaming
    /// bubble tagged with `iteration` is started. An empty placeholder (the
    /// one created on `run_accepted` before the first turn) is reused — just
    /// stamped with the iteration — to avoid an empty step bubble.
    pub fn begin_step(&self, run_id: &str, iteration: usize) {
        let target_id = format!("assistant-{}", run_id);
        self.messages.update(|msgs| {
            let len = msgs.len();
            if let Some(idx) = msgs.iter().rposition(|m| m.id == target_id) {
                let has_payload =
                    !msgs[idx].content.is_empty() || !msgs[idx].tool_calls.is_empty();
                if has_payload {
                    msgs[idx].is_streaming = false;
                    msgs[idx].is_intermediate = true;
                    msgs[idx].id = format!("intermediate-{}-{}", run_id, len);
                    msgs.push(ChatMessage {
                        id: target_id,
                        role: "assistant".into(),
                        content: String::new(),
                        tool_calls: vec![],
                        is_streaming: true,
                        is_intermediate: false,
                        error: None,
                        model_info: None,
                        iteration: Some(iteration),
                        timestamp: Some(super::timeline::now_millis()),
                    });
                } else {
                    msgs[idx].iteration = Some(iteration);
                }
            }
        });
        self.phase.set(ChatPhase::Thinking);
    }
```

- [ ] **Step 7: 运行测试，确认通过**

Run: `cargo test -p aleph-panel step_tests`
Expected: PASS（2 tests）

- [ ] **Step 8: 写失败测试 —— `set_step_text` 覆盖预览 + 命中已定格步骤**

在 `step_tests` 模块内追加：

```rust
    #[test]
    fn set_step_text_overwrites_streamed_preview() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);
        chat.append_chunk("r1", "par");
        chat.append_chunk("r1", "tial");
        chat.set_step_text("r1", 1, "authoritative");

        let content = chat.messages.with(|m| m[0].content.clone());
        assert_eq!(content, "authoritative");
    }

    #[test]
    fn set_step_text_targets_finalized_step_by_iteration() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1");
        chat.begin_step("r1", 1);
        chat.append_chunk("r1", "x");
        chat.begin_step("r1", 2); // finalizes step 1 as intermediate
        chat.set_step_text("r1", 1, "late fix");

        let content = chat.messages.with(|m| m[0].content.clone());
        assert_eq!(content, "late fix");
    }
```

- [ ] **Step 9: 运行测试，确认失败**

Run: `cargo test -p aleph-panel step_tests`
Expected: FAIL（`set_step_text` 未定义）

- [ ] **Step 10: 实现 `set_step_text`**

在 `begin_step` 之后加入：

```rust
    /// Set the authoritative text for the bubble of `run_id` at `iteration`,
    /// overwriting any streamed typewriter preview. Targets the bubble
    /// carrying the matching iteration tag — the live `assistant-{run_id}`
    /// bubble or an already-finalized `intermediate-{run_id}-{n}` bubble for
    /// this run — so late `text_emitted` events still land correctly.
    pub fn set_step_text(&self, run_id: &str, iteration: usize, text: &str) {
        let assistant_id = format!("assistant-{}", run_id);
        let intermediate_prefix = format!("intermediate-{}-", run_id);
        self.messages.update(|msgs| {
            if let Some(m) = msgs.iter_mut().rev().find(|m| {
                m.iteration == Some(iteration)
                    && (m.id == assistant_id || m.id.starts_with(&intermediate_prefix))
            }) {
                m.content = text.to_string();
            }
        });
    }
```

- [ ] **Step 11: 运行全部 step 测试，确认通过**

Run: `cargo test -p aleph-panel step_tests`
Expected: PASS（4 tests）

- [ ] **Step 12: Commit**

```bash
git add interfaces/webchat/src/views/chat/state.rs \
        interfaces/webchat/src/components/chat_sidebar.rs \
        interfaces/webchat/src/views/chat/timeline.rs \
        interfaces/webchat/src/components/workspace_panel.rs
git commit -m "panel: add ChatMessage.iteration + begin_step/set_step_text"
```

---

## Task 2: WorkspaceState 跨高亮焦点 + 当前迭代

**Files:**
- Modify: `interfaces/webchat/src/state/layout.rs`
- Test: `interfaces/webchat/src/state/layout.rs`（既有 `#[cfg(test)] mod tests`）

- [ ] **Step 1: 加字段到 `WorkspaceState`**

在 `state/layout.rs` `WorkspaceState` 结构体内（`selected_file` 字段之后）追加：

```rust
    /// Cross-highlight focus: the `(run_id, iteration)` step the user clicked
    /// on either surface. Both the chat bubble and the timeline step card read
    /// this to render a highlight ring. Cleared on reset.
    pub focused_step: RwSignal<Option<(String, usize)>>,
    /// Iteration of the currently-active turn (set on `agent_trace.turn_started`).
    /// Lets the timeline mark the live step. Cleared on reset.
    pub current_iteration: RwSignal<Option<usize>>,
```

- [ ] **Step 2: 在 `new()` 初始化新字段**

在 `WorkspaceState::new()` 的 `Self { ... }` 内（`selected_file` 之后）加：

```rust
            focused_step: RwSignal::new(None),
            current_iteration: RwSignal::new(None),
```

- [ ] **Step 3: 在测试 helper `test_ws` 初始化新字段**

在 `#[cfg(test)] mod tests` 的 `test_ws` 内 `Self { ... }`（`selected_file` 之后）加同样两行：

```rust
            focused_step: RwSignal::new(None),
            current_iteration: RwSignal::new(None),
```

- [ ] **Step 4: 在 `reset()` 清理新字段**

在 `reset()` 末尾（`self.selected_file.set(None);` 之后）加：

```rust
        self.focused_step.set(None);
        self.current_iteration.set(None);
```

- [ ] **Step 5: 编译确认字段已就绪**

Run: `cargo check -p aleph-panel`
Expected: PASS

- [ ] **Step 6: 写失败测试 —— focus_step / is_step_focused / set_current_iteration**

在 `state/layout.rs` 的 `#[cfg(test)] mod tests` 内追加：

```rust
    #[test]
    fn focus_step_sets_focus_and_opens_split() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::ChatOnly);
        ws.focus_step("run-1", 2);
        assert!(ws.is_step_focused("run-1", 2));
        assert_eq!(ws.mode.get_untracked(), LayoutMode::Split);
    }

    #[test]
    fn is_step_focused_discriminates_run_and_iteration() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        ws.focus_step("run-1", 2);
        assert!(ws.is_step_focused("run-1", 2));
        assert!(!ws.is_step_focused("run-1", 3));
        assert!(!ws.is_step_focused("run-2", 2));
    }

    #[test]
    fn set_current_iteration_tracks_active_turn() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        ws.set_current_iteration(3);
        assert_eq!(ws.current_iteration.get_untracked(), Some(3));
    }

    #[test]
    fn reset_clears_focus_and_current_iteration() {
        let owner = Owner::new();
        owner.set();
        let ws = test_ws(LayoutMode::Split);
        ws.focus_step("run-1", 1);
        ws.set_current_iteration(5);
        ws.reset();
        assert!(!ws.is_step_focused("run-1", 1));
        assert_eq!(ws.current_iteration.get_untracked(), None);
    }
```

- [ ] **Step 7: 运行测试，确认失败**

Run: `cargo test -p aleph-panel --lib state::layout`
Expected: FAIL（`focus_step` / `is_step_focused` / `set_current_iteration` 未定义）

- [ ] **Step 8: 实现三个方法**

在 `WorkspaceState` impl 内（`get_tool_payload` 之后）加入：

```rust
    /// Record the active turn's iteration (`agent_trace.turn_started`).
    pub fn set_current_iteration(&self, iteration: usize) {
        self.current_iteration.set(Some(iteration));
    }

    /// Focus a step for cross-highlight. Opens Split if not already (so a
    /// chat-side click reveals the timeline group), mirroring `focus_tool_row`.
    pub fn focus_step(&self, run_id: impl Into<String>, iteration: usize) {
        self.focused_step.set(Some((run_id.into(), iteration)));
        if self.mode.get_untracked() != LayoutMode::Split {
            self.set_layout(LayoutMode::Split);
        }
    }

    /// True when `(run_id, iteration)` is the focused step.
    pub fn is_step_focused(&self, run_id: &str, iteration: usize) -> bool {
        self.focused_step
            .with(|f| f.as_ref().is_some_and(|(r, i)| r == run_id && *i == iteration))
    }
```

- [ ] **Step 9: 运行测试，确认通过**

Run: `cargo test -p aleph-panel --lib state::layout`
Expected: PASS（含既有 + 4 个新测试）

- [ ] **Step 10: Commit**

```bash
git add interfaces/webchat/src/state/layout.rs
git commit -m "panel: add WorkspaceState focused_step + current_iteration"
```

---

## Task 3: 事件接线 —— agent_trace 驱动分步 + response_chunk 预览规则

**Files:**
- Modify: `interfaces/webchat/src/views/chat/events.rs`

> 本任务为接线，逻辑已被 Task 1/2 的方法单测覆盖。验证以 `cargo check` + `cargo test`（确保无回归）为准。

- [ ] **Step 1: 在 `agent_trace` 的 `kind` match 内加 `turn_started` / `text_emitted` 分支**

在 `events.rs` 的 `match kind { ... }` 内，`"tool_summary" => { ... }` 分支之后、`_ => {}` 之前插入：

```rust
                    "turn_started" => {
                        let iteration = trace_event
                            .get("iteration")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize;
                        chat.begin_step(run_id, iteration);
                        workspace.set_current_iteration(iteration);
                    }
                    "text_emitted" => {
                        let iteration = trace_event
                            .get("iteration")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as usize;
                        let text = trace_event
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        chat.set_step_text(run_id, iteration, text);
                    }
```

- [ ] **Step 2: 改写 `response_chunk` 分支**

将 `events.rs` 当前的整个 `"response_chunk" => { ... }` 分支替换为：

```rust
            "response_chunk" => {
                // Live typewriter preview only. Authoritative per-step text
                // arrives via `agent_trace.text_emitted` (both output modes),
                // overwriting this preview. `is_final` chunks are ignored: in
                // instant mode that delta is the whole-run buffered dump, and
                // appending it would duplicate text already set per step.
                let is_final = data
                    .get("is_final")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !is_final {
                    let chunk_text = data
                        .get("delta")
                        .or_else(|| data.get("content"))
                        .and_then(|c| c.as_str());
                    if let Some(text) = chunk_text {
                        chat.append_chunk(run_id, text);
                    }
                }
            }
```

- [ ] **Step 3: 编译验证**

Run: `cargo check -p aleph-panel`
Expected: PASS

- [ ] **Step 4: 回归测试无破坏**

Run: `cargo test -p aleph-panel`
Expected: PASS（全部既有 + 新测试）

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/chat/events.rs
git commit -m "panel: drive step segmentation from agent_trace, demote response_chunk to preview"
```

---

## Task 4: 右侧时间线按迭代分组（StepCard）

**Files:**
- Modify: `interfaces/webchat/src/components/workspace_panel.rs`
- Modify: `interfaces/webchat/src/views/chat/messages.rs`（把 `run_id_from_message_id` 提为 `pub(crate)`，见 Step 1）
- Test: `interfaces/webchat/src/components/workspace_panel.rs`（既有 `#[cfg(test)] mod`）

- [ ] **Step 1: 把 `run_id_from_message_id` 升级并提升可见性（messages.rs）**

将 `messages.rs:223` 的 `fn run_id_from_message_id` 整体替换为（既支持 `assistant-`，也支持 `intermediate-{run}-{n}`，并 `pub(crate)` 供 workspace_panel 复用）：

```rust
/// Recover the run id from a message id. Handles both the live
/// `assistant-{run}` id and finalized `intermediate-{run}-{n}` step ids.
/// Returns the id unchanged for user messages.
pub(crate) fn run_id_from_message_id(message_id: &str) -> String {
    if let Some(r) = message_id.strip_prefix("assistant-") {
        return r.to_string();
    }
    if let Some(rest) = message_id.strip_prefix("intermediate-") {
        return match rest.rfind('-') {
            Some(pos) => rest[..pos].to_string(),
            None => rest.to_string(),
        };
    }
    message_id.to_string()
}
```

- [ ] **Step 2: 写失败测试 —— `timeline_groups` 分组**

在 `workspace_panel.rs` 的 `#[cfg(test)] mod tests` 内追加（沿用既有 `msg_with_tools` helper 风格；如需 iteration 字段已在 Task 1 补上）：

```rust
    use crate::views::chat::state::{ChatMessage, ChatState};

    fn step_msg(id: &str, iteration: Option<usize>, content: &str, tools: Vec<ToolCallEntry>) -> ChatMessage {
        ChatMessage {
            id: id.to_string(),
            role: "assistant".into(),
            content: content.to_string(),
            tool_calls: tools,
            is_streaming: false,
            is_intermediate: false,
            error: None,
            model_info: None,
            iteration,
            timestamp: None,
        }
    }

    #[test]
    fn timeline_groups_one_per_tagged_bubble() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.messages.set(vec![
            step_msg("intermediate-r1-1", Some(1), "search images", vec![ToolCallEntry {
                tool_id: "t1".into(), tool_name: "search".into(), status: "completed".into(), duration_ms: Some(5),
            }]),
            step_msg("assistant-r1", Some(2), "write html", vec![ToolCallEntry {
                tool_id: "t2".into(), tool_name: "write".into(), status: "running".into(), duration_ms: None,
            }]),
        ]);

        let groups = timeline_groups(&chat);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].run_id, "r1");
        assert_eq!(groups[0].iteration, 1);
        assert_eq!(groups[0].narration, "search images");
        assert_eq!(groups[0].tools, vec![("t1".to_string(), "search".to_string())]);
        assert_eq!(groups[1].iteration, 2);
        assert_eq!(groups[1].tools, vec![("t2".to_string(), "write".to_string())]);
    }

    #[test]
    fn timeline_groups_skips_untagged_messages() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.messages.set(vec![step_msg("assistant-r1", None, "no tag", vec![])]);
        assert!(timeline_groups(&chat).is_empty());
    }
```

- [ ] **Step 3: 运行测试，确认失败**

Run: `cargo test -p aleph-panel --lib workspace_panel`
Expected: FAIL（`timeline_groups` / `StepGroup` 未定义）

- [ ] **Step 4: 实现 `StepGroup` + `timeline_groups`，移除旧 `timeline_rows`**

在 `workspace_panel.rs` 顶部 imports 后，删除旧 `fn timeline_rows`（约 23-35 行），替换为：

```rust
use crate::views::chat::messages::run_id_from_message_id;

/// One agent step (Think→Act iteration) for the workspace timeline: the
/// iteration's narration plus the tool calls it triggered.
#[derive(Clone, PartialEq)]
struct StepGroup {
    run_id: String,
    iteration: usize,
    narration: String,
    tools: Vec<(String, String)>, // (tool_id, tool_name)
}

/// Build iteration-grouped steps from the chat transcript. Each assistant
/// bubble carrying an `iteration` tag becomes one step; its content is the
/// narration and its `tool_calls` are the step's tools.
fn timeline_groups(chat: &ChatState) -> Vec<StepGroup> {
    chat.messages
        .get()
        .iter()
        .filter(|m| m.role == "assistant")
        .filter_map(|m| {
            let iteration = m.iteration?;
            Some(StepGroup {
                run_id: run_id_from_message_id(&m.id),
                iteration,
                narration: m.content.clone(),
                tools: m
                    .tool_calls
                    .iter()
                    .map(|t| (t.tool_id.clone(), t.tool_name.clone()))
                    .collect(),
            })
        })
        .collect()
}
```

- [ ] **Step 5: 运行测试，确认通过**

Run: `cargo test -p aleph-panel --lib workspace_panel`
Expected: PASS（2 个新测试 + 既有）

- [ ] **Step 6: 用 `StepCard` 重写 `ActivityTimeline`，新增 `StepCard` 组件**

将 `ActivityTimeline` 组件整体替换为下面两段（`ActivityRow` / `PayloadBlock` / `WorkspaceEmptyHero` 保持不动，被复用）：

```rust
/// The reactive activity timeline — one card per agent step (iteration).
#[component]
fn ActivityTimeline() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let groups = Memo::new(move |_| timeline_groups(&chat));

    move || {
        let data = groups.get();
        if data.is_empty() {
            view! { <WorkspaceEmptyHero /> }.into_any()
        } else {
            view! {
                <div class="flex flex-col gap-3">
                    {data
                        .into_iter()
                        .map(|g| view! { <StepCard group=g /> })
                        .collect_view()}
                </div>
            }
            .into_any()
        }
    }
}

/// One agent step: iteration header + narration + its tool rows. Clicking the
/// header focuses the step (cross-highlight with the chat bubble).
#[component]
fn StepCard(group: StepGroup) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();

    let run_id = group.run_id.clone();
    let iteration = group.iteration;
    let run_for_focus = run_id.clone();
    let run_for_highlight = run_id.clone();

    let focused = Memo::new(move |_| workspace.is_step_focused(&run_for_highlight, iteration));
    let active = Memo::new(move |_| workspace.current_iteration.get() == Some(iteration));

    let narration = group.narration.clone();
    let has_narration = !narration.is_empty();
    let tools = group.tools.clone();

    view! {
        <div
            id=format!("group-{}-{}", run_id, iteration)
            class=move || {
                let base = "rounded-lg border p-2 flex flex-col gap-2 transition-colors";
                if focused.get() {
                    format!("{base} border-primary/60 bg-primary/5")
                } else {
                    format!("{base} border-border/50 bg-surface-sunken/30")
                }
            }
        >
            <button
                type="button"
                class="flex items-center gap-2 text-left"
                on:click=move |_| workspace.focus_step(run_for_focus.clone(), iteration)
            >
                <span class="text-[10px] font-mono uppercase tracking-wider text-text-tertiary">
                    {format!("#{iteration}")}
                </span>
                <Show when=move || active.get()>
                    <span class="inline-block w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span>
                </Show>
            </button>
            <Show when=move || has_narration>
                <p class="text-xs text-text-secondary whitespace-pre-wrap leading-relaxed">
                    {narration.clone()}
                </p>
            </Show>
            <div class="flex flex-col gap-2">
                {tools
                    .clone()
                    .into_iter()
                    .map(|(tool_id, tool_name)| {
                        view! {
                            <ActivityRow run_id=run_id.clone() tool_id=tool_id tool_name=tool_name />
                        }
                    })
                    .collect_view()}
            </div>
        </div>
    }
}
```

- [ ] **Step 7: 编译验证（render 改动）**

Run: `cargo check -p aleph-panel`
Expected: PASS（若报 `file_path_of` 未用，确认仍被 `ActivityRow` 使用；无未用 import 警告）

- [ ] **Step 8: Commit**

```bash
git add interfaces/webchat/src/components/workspace_panel.rs \
        interfaces/webchat/src/views/chat/messages.rs
git commit -m "panel: render workspace timeline as iteration-grouped step cards"
```

---

## Task 5: 左侧气泡迭代标签 + 跨高亮

**Files:**
- Modify: `interfaces/webchat/src/views/chat/messages.rs`
- Test: `interfaces/webchat/src/views/chat/messages.rs`（`#[cfg(test)] mod`）

- [ ] **Step 1: 写失败测试 —— `run_id_from_message_id` 处理 intermediate 前缀**

在 `messages.rs` 的 `#[cfg(test)] mod tests` 内追加（若无 tests 模块则新建）：

```rust
#[cfg(test)]
mod run_id_tests {
    use super::run_id_from_message_id;

    #[test]
    fn strips_assistant_and_intermediate_prefixes() {
        assert_eq!(run_id_from_message_id("assistant-r1"), "r1");
        assert_eq!(run_id_from_message_id("intermediate-r1-3"), "r1");
        assert_eq!(run_id_from_message_id("intermediate-run-x-7"), "run-x");
        assert_eq!(run_id_from_message_id("user-0"), "user-0");
    }
}
```

- [ ] **Step 2: 运行测试，确认通过（Task 4 Step 1 已实现）**

Run: `cargo test -p aleph-panel --lib run_id_tests`
Expected: PASS

> 说明：升级逻辑在 Task 4 Step 1 已落地，此测试锁定其行为契约。若先执行 Task 5，请先做 Task 4 Step 1。

- [ ] **Step 3: 计算焦点态与反应式气泡 class（messages.rs `MessageBubble`）**

在 `MessageBubble` 内、`let message_run_id = run_id_from_message_id(&message.id);`（约 280 行）之后插入：

```rust
    let msg_iteration = message.iteration;
    let focused = {
        let run = message_run_id.clone();
        Memo::new(move |_| match (workspace, msg_iteration) {
            (Some(ws), Some(it)) => ws.is_step_focused(&run, it),
            _ => false,
        })
    };
    let bubble_base = bubble_class.clone();
    let bubble_class_reactive = move || {
        if focused.get() {
            format!("{bubble_base} ring-2 ring-primary/60")
        } else {
            bubble_base.clone()
        }
    };
    let bubble_dom_id = match (msg_iteration, is_user) {
        (Some(it), false) => format!("step-{message_run_id}-{it}"),
        _ => String::new(),
    };
    let iteration_label = match (workspace, msg_iteration, is_user) {
        (Some(ws), Some(it), false) => {
            let run = message_run_id.clone();
            Some(view! {
                <button
                    type="button"
                    class="mb-1 text-[10px] font-mono uppercase tracking-wider
                           text-text-tertiary hover:text-primary transition-colors"
                    on:click=move |_| ws.focus_step(run.clone(), it)
                >
                    {format!("#{it}")}
                </button>
            })
        }
        _ => None,
    };
```

- [ ] **Step 4: 在 view 中应用反应式 class、dom id 与迭代标签**

把 `MessageBubble` 的 `view!` 里这一行（约 434 行）：

```rust
            <div class=bubble_class>
                {tool_calls_view}
```

替换为：

```rust
            <div class=bubble_class_reactive id=bubble_dom_id>
                {iteration_label}
                {tool_calls_view}
```

- [ ] **Step 5: 编译验证**

Run: `cargo check -p aleph-panel`
Expected: PASS（确认 `bubble_class` 不再有其它用处而产生未用变量警告——它已被 `bubble_base` clone 接管；如有警告，把原 `let bubble_class = ...` 保留即可，因为 `bubble_base` 由它克隆）

- [ ] **Step 6: 运行全量测试**

Run: `cargo test -p aleph-panel`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/views/chat/messages.rs
git commit -m "panel: add iteration label + cross-highlight ring to chat bubbles"
```

---

## Task 6: WASM 打包 + 手动联调验证

**Files:** 无（构建 + 验证）

- [ ] **Step 1: 全量主机测试**

Run: `cargo test -p aleph-panel`
Expected: PASS

- [ ] **Step 2: 构建 WASM 包**

Run: `just wasm`
Expected: 成功生成 `interfaces/webchat/dist/{aleph_panel.js, aleph_panel_bg.wasm, tailwind.css, index.html}`

- [ ] **Step 3: 重编并热替换 daemon（让 rust_embed 烧入新 dist）**

按 CLAUDE.md "Panel ↔ Daemon 资源嵌入链"：

```bash
cargo build --release -p alephcore --bin aleph-server
./target/release/aleph-server stop || true
cargo run --release -p alephcore --bin aleph-server start
```

- [ ] **Step 4: 手动验证清单（打字机模式）**

在 Panel 设置里把 output mode 设为 typewriter，发一条会触发多步工具调用的指令，确认：
- 左侧：每一步叙述逐字打出，落在独立气泡，气泡顶部有 `#N` 迭代标签。
- 右侧（Split）：每步一张 StepCard，含 `#N` + 叙述 + 该步工具行，可展开看 args/result。
- 点左侧某步 `#N` → 右侧对应 StepCard 高亮（primary ring）+ 自动进 Split。
- 点右侧某 StepCard `#N` → 左侧对应气泡高亮。

- [ ] **Step 5: 手动验证清单（即时模式）**

把 output mode 切到 instant，重发类似指令，确认：
- 左侧：每步叙述在该轮完成时整段出现（非逐字），仍按步分气泡、带 `#N` 标签。
- 右侧 StepCard 分组正确、工具 args/result 可见。
- 不出现"整段拼接 dump"重复文本。
- 跨高亮双向有效。

- [ ] **Step 6: 最终提交（如有 dist 产物纳入版本库的约定则提交，否则跳过）**

```bash
git add -A
git commit -m "panel: workflow echo × workspace timeline integration (wasm build)"
```

> 注：若仓库不跟踪 `interfaces/webchat/dist/`，Step 6 仅提交代码改动（前面任务已分别提交，此步可能无内容，跳过即可）。

---

## Self-Review

**1. Spec coverage（逐条对照 spec）：**
- §3.1 路线 A（前端 + agent_trace 驱动）→ Task 3 接线、Task 4/5 渲染。✅
- §3.2 权威=agent_trace（turn_started 切步 / text_emitted 填文 / tool 挂步）→ Task 1（begin_step/set_step_text）+ Task 3。✅
- §3.2 response_chunk=预览、即时忽略 dump → Task 3 Step 2。✅
- §4.1 数据模型 `ChatMessage.iteration` + `focused_step` + `current_iteration` → Task 1 / Task 2。✅（spec 的 `step_narration`/`tool_iteration` 映射按 spec 明示的"或在 timeline 构建时从 ChatState 推导"选项实现为 `timeline_groups`，更 DRY——见 Task 4。）
- §4.3 右侧按迭代分组 + 叙述 + 工具行 → Task 4。✅
- §4.4 左侧迭代标签 + tool chip 保留 → Task 5（chip 渲染未动）。✅
- §4.5 cross-highlight 双向（focused_step 驱动高亮）→ Task 4（右）+ Task 5（左）。✅
- §5 边缘情况：无工具迭代（StepCard `has_narration` 控制、tools 空列表自然无行）、无叙述迭代（`has_narration=false` 隐藏段落）、打字机漂移自愈（set_step_text 覆盖）、即时 dump 忽略（Task 3）、reset 清理（Task 2 Step 4）。✅
- §7 测试：turn_started 分步、text_emitted 覆盖、focused_step 双向、分组、reset → Task 1/2/4 单测覆盖；即时模式无 response_chunk 增量即可构出分步 → 由 timeline_groups 仅依赖 chat.messages（agent_trace 填充）保证 + Task 6 Step 5 手动验证。✅

**偏离说明（已获 spec 授权）：** spec §4.1 把"tool→iteration 归属"列为"映射 **或** 从 ChatState 推导"两选一。本计划选后者：右侧 `timeline_groups` 直接从 `chat.messages` 的气泡（已携带 iteration + content + tool_calls）派生，省去 `step_narration` / `tool_iteration` 两个 WorkspaceState 映射，避免双写同一事实（DRY）。`scrollIntoView`（spec §4.5 附带项）本版仅实现高亮环（满足"互相高亮"核心诉求），滚动留作后续增强，避免 host 编译引入 web_sys DOM 调用。

**2. Placeholder scan：** 无 TBD/TODO/"类似 Task N"；每个改码步骤均含完整代码块。✅

**3. Type consistency：**
- `begin_step(&self, run_id: &str, iteration: usize)` / `set_step_text(&self, run_id: &str, iteration: usize, text: &str)`：Task 1 定义，Task 3 调用签名一致。✅
- `focus_step(impl Into<String>, usize)` / `is_step_focused(&str, usize)` / `set_current_iteration(usize)`：Task 2 定义，Task 3/4/5 调用一致。✅
- `StepGroup { run_id, iteration, narration, tools: Vec<(String,String)> }`：Task 4 定义并在同任务渲染、测试中一致使用。✅
- `run_id_from_message_id`：Task 4 Step 1 升级为 `pub(crate)`，Task 4（workspace_panel）与 Task 5（messages 自身 + 测试）均引用同一签名。✅
- `ChatMessage.iteration: Option<usize>`：Task 1 定义，Task 4 测试 helper / Task 5 读取一致。✅

---

## Execution Handoff

计划已保存。两种执行方式：

1. **Subagent-Driven（推荐）** —— 每个 Task 派新 subagent 实现，任务间审查，快速迭代。
2. **Inline Execution** —— 本会话内按 executing-plans 分批执行，带检查点审查。
