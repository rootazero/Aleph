# 流式回显与工作区面板重构 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 聊天列改为「叙述为主线 + 单行工具条目 + 探索聚合块」的 claude-code 式 transcript（叙述常驻不再被折叠吞掉），右侧工作区从重复时间线改为工具详情查看器，prompt 层微调叙述指令。

**Architecture:** 数据链路（后端事件、`events.rs` 投影、`ChatState` 消息模型）不动；集中重写前端派生层（`timeline.rs` 纯函数行模型）与渲染层（`messages.rs`/`tool_card.rs`/`workspace_panel.rs`），`WorkspaceState` 以 `selected_tool` 取代 step 粒度交叉高亮。对应 spec：`docs/superpowers/specs/2026-07-04-streaming-echo-workspace-redesign-design.md`。

**Tech Stack:** Rust + Leptos 0.7 (WASM Panel，crate 名 `aleph-panel`，目录 `interfaces/webchat/`)；prompt 层在主 crate `alephcore`。

## Global Constraints

- **cargo 路径**：本机 cargo 不在 PATH（rustup symlink 坏）。所有 cargo 命令前加：`export PATH="$HOME/.rustup/toolchains/1.96.0-aarch64-apple-darwin/bin:$PATH"`
- **测试命令**：panel 侧统一 `cargo test -p aleph-panel --lib <filter>`（host 单测，无需 WASM）；prompt 侧 `cargo test -p alephcore --lib multi_step_conduct`。节制 cargo：每步只跑给出的过滤命令，不跑全量。
- **提交规范**：English message，`<scope>: <description>`（scope 用 `panel:` / `thinker:` / `docs:`），无 attribution 尾注。
- **i18n**：新词条必须同时加 `interfaces/webchat/locales/zh.json` 与 `en.json`（缺一编译失败）。
- **Panel↔Daemon 嵌入链**：改 panel 后要看真实效果需 `just wasm` → 重编 server binary（rust_embed 编译期嵌入）。仅最终验证任务做一次。
- **架构红线**：不碰 `src/harness/`；Panel 不做业务推理（R4/R10）。
- **风格**：文件内注释风格 = 中文模块注释 + 英文行内（跟随各文件现状）；不重排既有代码。

---

### Task 1: `ToolCallEntry.started_at_ms` — 长静默耗时的数据基础

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/state.rs:248-255`（struct）、`state.rs:802-826`（update_tool）
- Test: 同文件 `#[cfg(test)]` 模块（文件尾部已有 tests mod）

**Interfaces:**
- Produces: `ToolCallEntry.started_at_ms: Option<i64>`（epoch ms，首次 running 时间戳）。Task 4 的耗时显示、Task 2 的行模型都携带整个 `ToolCallEntry`。

- [ ] **Step 1: 写失败测试**

在 `state.rs` 尾部 tests 模块（若无合适 mod 就新增 `mod tool_timestamp_tests`）加：

```rust
#[cfg(test)]
mod tool_timestamp_tests {
    use super::*;

    #[test]
    fn update_tool_stamps_started_at_on_first_running() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.start_assistant_message("r1"); // 建 assistant-r1 容器（若无此方法，用 messages.set 手工放一条 id="assistant-r1" 的 assistant 消息）
        chat.update_tool("r1", "t1", "bash", "running", None);
        let started = chat.messages.with_untracked(|m| {
            m.iter()
                .flat_map(|m| m.tool_calls.iter())
                .find(|t| t.tool_id == "t1")
                .and_then(|t| t.started_at_ms)
        });
        assert!(started.is_some(), "first running must stamp started_at_ms");

        // 完成时不覆盖时间戳
        chat.update_tool("r1", "t1", "bash", "completed", Some(30));
        let after = chat.messages.with_untracked(|m| {
            m.iter()
                .flat_map(|m| m.tool_calls.iter())
                .find(|t| t.tool_id == "t1")
                .map(|t| (t.started_at_ms, t.status.clone()))
        });
        assert_eq!(after.map(|(s, _)| s), Some(started));
    }
}
```

注意：先查 `start_assistant_message` 是否存在且签名匹配（`grep -n "fn start_assistant_message" state.rs`）；不匹配就按文件里其他测试的建消息方式手工构造。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib tool_timestamp -- --nocapture`
Expected: FAIL（`started_at_ms` 字段不存在 → 编译错误即算失败）

- [ ] **Step 3: 最小实现**

`ToolCallEntry` 加字段（保持 serde 兼容）：

```rust
pub struct ToolCallEntry {
    pub tool_id: String,
    pub tool_name: String,
    pub status: String, // "running" | "completed" | "failed"
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Epoch-ms when the tool first went "running" — drives the live
    /// elapsed timer on long-running tool rows. Stamped panel-side.
    #[serde(default)]
    pub started_at_ms: Option<i64>,
}
```

`update_tool` 里：已有分支 `tc.status = ...` 不动时间戳；新建分支 push 时：

```rust
msg.tool_calls.push(ToolCallEntry {
    tool_id: tool_id.to_string(),
    tool_name: tool_name.to_string(),
    status: status.to_string(),
    duration_ms,
    started_at_ms: (status == "running")
        .then(super::timeline::now_millis),
});
```

全仓修编译错：`ToolCallEntry { ... }` 字面量构造点（`grep -rn "ToolCallEntry {" interfaces/webchat/src`，主要是各测试 fixture 与 `events.rs` 若有）逐个补 `started_at_ms: None`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib tool_timestamp`
Expected: PASS（同时 `cargo test -p aleph-panel --lib state` 无回归）

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/state.rs
git commit -m "panel: stamp tool start time for live elapsed display"
```

---

### Task 2: `timeline.rs` 新行模型 — 叙述行 / 工具行 / 探索聚合（核心派生）

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/timeline.rs`（整个 `TimelineRow` + `build_rows` + `row_key` + tests）

**Interfaces:**
- Consumes: `ChatMessage`（含 Task 1 的 `ToolCallEntry`）、`crate::components::tool_card::ToolKind::from_name`
- Produces（Task 5 渲染层消费，签名必须一致）:

```rust
pub enum TimelineRow {
    DaySeparator { key: String, label: String },
    /// 用户消息、最终回答等真正的会话轮（含 clock）——保持现状。
    Message { message: ChatMessage, clock: String },
    /// 中间轮叙述文本：无框直排，常驻。
    Narration { message: ChatMessage },
    /// 单个非只读工具：一行条目。
    ToolLine { run_id: String, tool: ToolCallEntry },
    /// 连续只读工具（FileRead/Search）的聚合块。
    ExploreGroup {
        /// 稳定 key：`explore:{run_id}:{首个 tool_id}`
        key: String,
        run_id: String,
        tools: Vec<ToolCallEntry>,
        /// 无 running 工具且来源消息都不再 streaming。
        completed: bool,
    },
}
pub fn row_key(row: &TimelineRow) -> String;
pub fn derive_timeline(messages: &[ChatMessage], today: &str, yesterday: &str) -> Vec<TimelineRow>;
pub fn is_explore_tool(tool_name: &str) -> bool;
```

**派生规则**（写进 `build_rows` 的 step 处理分支，替换现有 `pending: Vec<ChatMessage>` 折叠）：

对每条 `is_step(m)` 的消息（`is_step`/`is_final_answer` 判定原样保留）：
1. 若换 run（与当前 open explore group 的 run 不同）→ 先 flush explore group。
2. 叙述：`!m.content.trim().is_empty()` **或**（`m.is_streaming && m.tool_calls.is_empty()`，空流式占位=光标行）→ flush explore group，emit `Narration { message: m.clone() }`。空且非流式 → 不发行（跳过占位噪音）。
3. 逐个 `m.tool_calls`：`is_explore_tool(&t.tool_name)` → 追加进 open explore group（无则新建，记 `key = format!("explore:{run}:{first_tool_id}")`，并累计 `streaming |= m.is_streaming`）；否则 → flush explore group，emit `ToolLine { run_id, tool: t.clone() }`。
4. 非 step 行（user/final/separator 路径）到来或消息列表结束 → flush explore group。
5. flush 时 `completed = !streaming && tools.iter().all(|t| t.status != "running")`。

`is_explore_tool`：

```rust
/// 只读探索类工具（读文件 / 搜索）→ 塌缩进 ExploreGroup（codex Exploring 同源）。
#[must_use]
pub fn is_explore_tool(tool_name: &str) -> bool {
    use crate::components::tool_card::ToolKind;
    matches!(
        ToolKind::from_name(tool_name),
        ToolKind::FileRead | ToolKind::Search
    )
}
```

`row_key` 新 arm（保持「易变字段折进 key」的既有约定）：

```rust
TimelineRow::Narration { message: m } => {
    format!("narr:{}:{}:{}", m.id, m.content.len(), m.is_streaming)
}
TimelineRow::ToolLine { run_id, tool } => format!(
    "tool:{run_id}:{}:{}:{:?}",
    tool.tool_id, tool.status, tool.duration_ms
),
TimelineRow::ExploreGroup { key, tools, completed, .. } => {
    let running = tools.iter().filter(|t| t.status == "running").count();
    format!("{key}:{}:{completed}:{running}", tools.len())
}
```

- [ ] **Step 1: 写失败测试**

删除旧 StepStrip 专属测试（`consecutive_intermediates_fold_into_one_strip`、`streaming_step_marks_strip_incomplete`、`trailing_tool_step_folds_into_strip_not_dangling`、`empty_placeholder_step_folds_and_keeps_strip_open`、`pre_stamped_placeholder_folds_into_strip_from_first_frame`、`pure_text_final_answer_stays_standalone`、`final_answer_with_tool_call_escapes_the_strip`、`single_turn_reply_has_no_strip`、`row_key_strip_changes_on_content_update`），替换为新语义测试（fixture 沿用文件里现有 `msg_*` helpers，`ToolCallEntry` 记得带 `started_at_ms: None`）：

```rust
fn tool(id: &str, name: &str, status: &str) -> crate::views::chat::state::ToolCallEntry {
    crate::views::chat::state::ToolCallEntry {
        tool_id: id.into(),
        tool_name: name.into(),
        status: status.into(),
        duration_ms: None,
        started_at_ms: None,
    }
}

fn msg_step_tools(id: &str, it: usize, content: &str, streaming: bool,
                  tools: Vec<crate::views::chat::state::ToolCallEntry>) -> ChatMessage {
    let mut m = msg_step(id, it, content, streaming);
    m.tool_calls = tools;
    m
}

#[test]
fn narration_then_tools_emit_in_order() {
    // 一个 step: 叙述 + 编辑工具 → Narration 行在前，ToolLine 在后
    let msgs = vec![
        msg_user("u1", "hi"),
        msg_step_tools("intermediate-r1-1", 1, "我先改配置", false,
                       vec![tool("t1", "file_edit", "completed")]),
        msg_final("r1", "done"),
    ];
    let rows = derive_timeline(&msgs, "Today", "Yesterday");
    let kinds: Vec<&str> = rows.iter().map(|r| match r {
        TimelineRow::Message { .. } => "msg",
        TimelineRow::Narration { .. } => "narr",
        TimelineRow::ToolLine { .. } => "tool",
        TimelineRow::ExploreGroup { .. } => "explore",
        TimelineRow::DaySeparator { .. } => "sep",
    }).collect();
    assert_eq!(kinds, vec!["msg", "narr", "tool", "msg"]);
}

#[test]
fn consecutive_readonly_tools_merge_across_steps() {
    // step1: read+search（无叙述文本，content 空非流式）；step2: 又一个 read
    // → 三个只读工具并进一个 ExploreGroup（跨消息，中间无叙述打断）
    let msgs = vec![
        msg_step_tools("intermediate-r1-1", 1, "", false,
                       vec![tool("t1", "file_read", "completed"),
                            tool("t2", "web_search", "completed")]),
        msg_step_tools("intermediate-r1-2", 2, "", false,
                       vec![tool("t3", "file_read", "completed")]),
    ];
    let rows = derive_timeline(&msgs, "Today", "Yesterday");
    let group = rows.iter().find_map(|r| match r {
        TimelineRow::ExploreGroup { key, tools, completed, .. } =>
            Some((key.clone(), tools.len(), *completed)),
        _ => None,
    }).expect("one explore group");
    assert_eq!(group.1, 3);
    assert!(group.2, "all terminal → completed");
    assert_eq!(group.0, "explore:r1:t1", "key anchors to first tool id");
}

#[test]
fn narration_flushes_explore_group() {
    // read → 叙述 → read ⇒ 两个 ExploreGroup，叙述行夹在中间
    let msgs = vec![
        msg_step_tools("intermediate-r1-1", 1, "", false,
                       vec![tool("t1", "file_read", "completed")]),
        msg_step_tools("intermediate-r1-2", 2, "找到了，接着看第二处", false,
                       vec![tool("t2", "file_read", "completed")]),
    ];
    let rows = derive_timeline(&msgs, "Today", "Yesterday");
    let kinds: Vec<&str> = rows.iter().map(|r| match r {
        TimelineRow::Narration { .. } => "narr",
        TimelineRow::ExploreGroup { .. } => "explore",
        _ => "other",
    }).collect();
    assert_eq!(kinds, vec!["explore", "narr", "explore"]);
}

#[test]
fn action_tool_flushes_explore_group() {
    let msgs = vec![msg_step_tools("intermediate-r1-1", 1, "", false, vec![
        tool("t1", "file_read", "completed"),
        tool("t2", "file_edit", "completed"),
        tool("t3", "file_read", "completed"),
    ])];
    let rows = derive_timeline(&msgs, "Today", "Yesterday");
    let kinds: Vec<&str> = rows.iter().map(|r| match r {
        TimelineRow::ExploreGroup { .. } => "explore",
        TimelineRow::ToolLine { .. } => "tool",
        _ => "other",
    }).collect();
    assert_eq!(kinds, vec!["explore", "tool", "explore"]);
}

#[test]
fn running_or_streaming_group_not_completed() {
    let msgs = vec![msg_step_tools("intermediate-r1-1", 1, "", true,
                                   vec![tool("t1", "file_read", "running")])];
    let rows = derive_timeline(&msgs, "Today", "Yesterday");
    let completed = rows.iter().find_map(|r| match r {
        TimelineRow::ExploreGroup { completed, .. } => Some(*completed),
        _ => None,
    });
    assert_eq!(completed, Some(false));
}

#[test]
fn empty_streaming_placeholder_emits_cursor_narration() {
    let msgs = vec![msg_empty_step("r1", 1)];
    let rows = derive_timeline(&msgs, "Today", "Yesterday");
    assert!(matches!(rows.as_slice(),
        [TimelineRow::Narration { message }] if message.is_streaming));
}

#[test]
fn empty_finished_step_emits_nothing() {
    let mut m = msg_empty_step("r1", 1);
    m.is_streaming = false;
    let rows = derive_timeline(&[m], "Today", "Yesterday");
    assert!(rows.is_empty());
}

#[test]
fn final_answer_and_user_stay_message_rows() {
    // 原 pure_text_final_answer_stays_standalone / final_answer_with_tool_call_escapes_the_strip
    // 的语义在新模型下保留：final answer 是 Message 行
    let mut answer = msg_tool_step("r-r", 2, "最终报告……", false);
    answer.is_final = true;
    let msgs = vec![msg_user("u1", "q"),
                    msg_step("intermediate-r-r-1", 1, "searching", false),
                    answer];
    let rows = derive_timeline(&msgs, "Today", "Yesterday");
    assert!(rows.iter().any(|r| matches!(r,
        TimelineRow::Message { message, .. }
            if message.id == "assistant-r-r" && !message.tool_calls.is_empty())));
}

#[test]
fn row_key_narration_changes_on_content_growth() {
    let m1 = msg_step("intermediate-r1-1", 1, "partial", true);
    let m2 = msg_step("intermediate-r1-1", 1, "partial more", true);
    assert_ne!(
        row_key(&TimelineRow::Narration { message: m1 }),
        row_key(&TimelineRow::Narration { message: m2 })
    );
}

#[test]
fn row_key_explore_changes_on_status_transition() {
    let g = |status: &str| TimelineRow::ExploreGroup {
        key: "explore:r1:t1".into(),
        run_id: "r1".into(),
        tools: vec![tool("t1", "file_read", status)],
        completed: status != "running",
    };
    assert_ne!(row_key(&g("running")), row_key(&g("completed")));
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib timeline`
Expected: FAIL（新变体不存在 → 编译错误）

- [ ] **Step 3: 实现**

按上方 Interfaces 与派生规则改 `TimelineRow` / `build_rows` / `row_key`，加 `is_explore_tool`。`build_rows` 内部用一个小的 builder 结构代替 `pending: Vec<ChatMessage>`：

```rust
/// Open explore-group accumulator (flushed on narration / action tool /
/// non-step row / end of input).
struct ExploreAcc {
    key: String,
    run_id: String,
    tools: Vec<crate::views::chat::state::ToolCallEntry>,
    streaming: bool,
}

fn flush_explore(rows: &mut Vec<TimelineRow>, acc: &mut Option<ExploreAcc>) {
    if let Some(a) = acc.take() {
        let completed = !a.streaming && a.tools.iter().all(|t| t.status != "running");
        rows.push(TimelineRow::ExploreGroup {
            key: a.key,
            run_id: a.run_id,
            tools: a.tools,
            completed,
        });
    }
}
```

step 分支主体（替换现有 `if is_step(m) { ... }` 内容）：

```rust
if is_step(m) {
    let run = run_id_of(m);
    if acc.as_ref().is_some_and(|a| a.run_id != run) {
        flush_explore(&mut rows, &mut acc);
    }
    let has_narration =
        !m.content.trim().is_empty() || (m.is_streaming && m.tool_calls.is_empty());
    if has_narration {
        flush_explore(&mut rows, &mut acc);
        rows.push(TimelineRow::Narration { message: m.clone() });
    }
    for t in &m.tool_calls {
        if is_explore_tool(&t.tool_name) {
            let a = acc.get_or_insert_with(|| ExploreAcc {
                key: format!("explore:{run}:{}", t.tool_id),
                run_id: run.clone(),
                tools: Vec::new(),
                streaming: false,
            });
            a.tools.push(t.clone());
            a.streaming |= m.is_streaming;
        } else {
            flush_explore(&mut rows, &mut acc);
            rows.push(TimelineRow::ToolLine {
                run_id: run.clone(),
                tool: t.clone(),
            });
        }
    }
    // Tool-carrying streaming step: keep the group "open" even if narration
    // was consumed above — streaming flag already folded per-push.
    if m.is_streaming && !m.tool_calls.is_empty() {
        if let Some(a) = acc.as_mut() { a.streaming = true; }
    }
    continue;
}
flush_explore(&mut rows, &mut acc); // 非 step 行关闭 open group
```

循环结束后 `flush_explore(&mut rows, &mut acc);`。模块头注释（1-15 行）同步改写为新行模型说明。删除 `flush_strip`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib timeline`
Expected: 新测试全 PASS。此时 `messages.rs`/`workspace_panel.rs` 会因 `StepStrip` 变体消失而编译失败——**预期内**，Task 5/7 修复；本任务只保证 `cargo test -p aleph-panel --lib timeline` 的编译单元先行正确性无法独立达成时，可临时在 `messages.rs` 的 match 加 `_ => view!{<span/>}.into_any()` 兜底 arm（Task 5 删除），保住绿灯。

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/timeline.rs interfaces/webchat/src/platform/wide/views/chat/messages.rs
git commit -m "panel: derive narration/tool-line/explore-group timeline rows"
```

---

### Task 3: `explore_entries` 合并纯函数（连续 Read 合并去重）

**Files:**
- Modify: `interfaces/webchat/src/components/tool_card.rs`（纯逻辑区，`summarize_tools` 附近）
- Test: 同文件 tests 模块

**Interfaces:**
- Consumes: `ToolKind::from_name`、`tool_headline`（调用方先算好 headline 传入）
- Produces（Task 5 的 ExploreGroup 展开体消费）:

```rust
/// 探索块展开体的一行：连续 FileRead 合并成一条（文件名去重连接），
/// Search 等其余只读工具各自一条。
#[derive(Debug, Clone, PartialEq)]
pub struct ExploreEntry {
    pub kind: ToolKind,
    /// 已合成的展示文案（如 "a.rs, b.rs" 或搜索词）。
    pub label: String,
    /// 该条覆盖的 tool_id（合并行含多个；点击取 first 进详情栏）。
    pub tool_ids: Vec<String>,
}
pub fn explore_entries(items: &[(String, String, Option<String>)]) -> Vec<ExploreEntry>;
// items: (tool_id, tool_name, headline)
```

- [ ] **Step 1: 写失败测试**

```rust
#[test]
fn explore_entries_merges_consecutive_reads_dedup() {
    let items = vec![
        ("t1".into(), "file_read".into(), Some("a.rs".to_string())),
        ("t2".into(), "file_read".into(), Some("b.rs".to_string())),
        ("t3".into(), "file_read".into(), Some("a.rs".to_string())), // dup 去重
        ("t4".into(), "web_search".into(), Some("panel bug".to_string())),
        ("t5".into(), "file_read".into(), Some("c.rs".to_string())), // search 打断后新起一条
    ];
    let entries = explore_entries(&items);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].label, "a.rs, b.rs");
    assert_eq!(entries[0].tool_ids, vec!["t1", "t2", "t3"]);
    assert_eq!(entries[1].kind, ToolKind::Search);
    assert_eq!(entries[1].label, "panel bug");
    assert_eq!(entries[2].label, "c.rs");
}

#[test]
fn explore_entries_headline_fallback_is_tool_name() {
    let items = vec![("t1".into(), "file_read".into(), None)];
    let entries = explore_entries(&items);
    assert_eq!(entries[0].label, "file_read");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib explore_entries`
Expected: FAIL（函数不存在）

- [ ] **Step 3: 实现**

```rust
#[must_use]
pub fn explore_entries(items: &[(String, String, Option<String>)]) -> Vec<ExploreEntry> {
    let mut out: Vec<ExploreEntry> = Vec::new();
    for (tool_id, name, headline) in items {
        let kind = ToolKind::from_name(name);
        let label = headline.clone().unwrap_or_else(|| name.clone());
        // 连续 FileRead 合并到上一条（label 去重后逗号连接）。
        if kind == ToolKind::FileRead {
            if let Some(last) = out.last_mut().filter(|e| e.kind == ToolKind::FileRead) {
                last.tool_ids.push(tool_id.clone());
                if !last.label.split(", ").any(|s| s == label) {
                    last.label.push_str(", ");
                    last.label.push_str(&label);
                }
                continue;
            }
        }
        out.push(ExploreEntry {
            kind,
            label,
            tool_ids: vec![tool_id.clone()],
        });
    }
    out
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib explore_entries`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/components/tool_card.rs
git commit -m "panel: merge consecutive reads into one explore entry"
```

---

### Task 4: 秒级时钟 + ToolCard 瘦身为单行条目（状态符 + 实时耗时）

**Files:**
- Create: `interfaces/webchat/src/state/run_clock.rs`
- Modify: `interfaces/webchat/src/state/mod.rs`（导出）、`interfaces/webchat/src/app.rs`（app root provide + 1s ticker，放在 TypewriterClock provide 旁，`grep -n "TypewriterClock" app.rs` 定位）、`interfaces/webchat/src/components/tool_card.rs`（ToolCard 视觉 + 状态符）
- Test: `run_clock.rs` 内嵌 tests

**Interfaces:**
- Produces:

```rust
/// 1s 粒度共享时钟（epoch ms）。仅 running 工具行订阅，避免全列表重渲染。
#[derive(Clone, Copy)]
pub struct SecondTick(pub RwSignal<i64>);
/// "12s" / "1m05s"。
pub fn fmt_elapsed(elapsed_ms: i64) -> String;
/// 显示耗时的静默阈值。
pub const LONG_RUN_THRESHOLD_MS: i64 = 8_000;
```

- ToolCard 视觉契约（Task 5/7 依赖）：外层无卡片边框（`glass-inset` 移除），头部行尾状态区 = running 时 `脉冲点 [+ 耗时(超8s)]`；completed 时 `✓`（`text-success`）+ 可选 `duration_ms` 灰字；failed 时 `✗`（`text-danger`）。其余（展开体、expanded_events 共享态、8 行封顶、溢出行）不变。

- [ ] **Step 1: 写 fmt_elapsed 失败测试**

`run_clock.rs`：

```rust
//! 1s shared clock for live tool-row elapsed timers + formatting helper.

use leptos::prelude::*;

#[derive(Clone, Copy)]
pub struct SecondTick(pub RwSignal<i64>);

pub const LONG_RUN_THRESHOLD_MS: i64 = 8_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_elapsed_seconds_and_minutes() {
        assert_eq!(fmt_elapsed(9_400), "9s");
        assert_eq!(fmt_elapsed(65_000), "1m05s");
        assert_eq!(fmt_elapsed(0), "0s");
        assert_eq!(fmt_elapsed(-5), "0s"); // 时钟回拨防御
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib run_clock`
Expected: FAIL（`fmt_elapsed` 未定义）

- [ ] **Step 3: 实现 fmt_elapsed + 模块接线**

```rust
/// "12s" / "1m05s" — 负值（时钟回拨）clamp 到 0。
#[must_use]
pub fn fmt_elapsed(elapsed_ms: i64) -> String {
    let secs = (elapsed_ms.max(0)) / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else {
        format!("{}m{:02}s", secs / 60, secs % 60)
    }
}
```

`state/mod.rs` 加 `pub mod run_clock;`。`app.rs` 在 TypewriterClock provide 处旁边加：

```rust
let sec_tick = RwSignal::new(crate::views::chat::timeline::now_millis());
provide_context(crate::state::run_clock::SecondTick(sec_tick));
#[cfg(target_arch = "wasm32")]
set_interval(
    move || sec_tick.set(crate::views::chat::timeline::now_millis()),
    std::time::Duration::from_secs(1),
);
```

（`set_interval` 的导入跟随 app.rs 里 30fps ticker 已用的同一 API；若它用的是 `set_interval_with_handle`，照抄该写法。）

Run: `cargo test -p aleph-panel --lib run_clock` → PASS

- [ ] **Step 4: ToolCard 瘦身（视觉，无新纯逻辑）**

`ToolCard` 组件改动（`tool_card.rs:429-477`）：

a. `status` Memo 扩展为同时取 `started_at_ms`：

```rust
let status = Memo::new(move |_| {
    chat.messages.get().iter()
        .flat_map(|m| m.tool_calls.iter())
        .find_map(|t| (t.tool_id == tid_for_status)
            .then(|| (t.status.clone(), t.duration_ms, t.started_at_ms)))
});
let running = move || matches!(status.get(), Some((s, _, _)) if s == "running");
let failed = move || matches!(status.get(), Some((s, _, _)) if s == "failed");
let succeeded = move || matches!(status.get(), Some((s, _, _)) if s == "completed");
```

b. 外层 div：`class="rounded-lg glass-inset hover:bg-surface-raised/30 transition-colors"` → `class="rounded-md hover:bg-surface-raised/40 transition-colors"`（去卡片 chrome，行化）。

c. 头部按钮状态区（现有 running 脉冲 `<Show>` 与 diff_stat 之间/之后）替换为三态：

先在组件体（`let kind = ...` 附近）取共享时钟：`let tick = use_context::<crate::state::run_clock::SecondTick>();`（`SecondTick` 是 Copy，可直接被闭包捕获）。然后：

```rust
// running: 脉冲点 + 超阈值实时耗时
<Show when=running>
    <span class="shrink-0 inline-block w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span>
    {
        let status = status;
        move || {
            match (tick, status.get()) {
                (Some(t), Some((_, _, Some(start)))) => {
                    let elapsed = t.0.get() - start;
                    (elapsed >= crate::state::run_clock::LONG_RUN_THRESHOLD_MS)
                        .then(|| view! {
                            <span class="shrink-0 text-[10px] font-mono text-text-tertiary tabular-nums">
                                {crate::state::run_clock::fmt_elapsed(elapsed)}
                            </span>
                        })
                }
                _ => None,
            }
        }
    }
</Show>
// completed: ✓ + 耗时
<Show when=succeeded>
    <span class="shrink-0 text-[11px] text-success">"✓"</span>
    {move || status.get().and_then(|(_, d, _)| d).map(|d| view! {
        <span class="shrink-0 text-[10px] font-mono text-text-tertiary">
            {crate::state::run_clock::fmt_elapsed(d as i64)}
        </span>
    })}
</Show>
// failed: ✗
<Show when=failed>
    <span class="shrink-0 text-[11px] text-danger">"✗"</span>
</Show>
```

注意 Leptos 闭包捕获：`status` 是 `Memo`（Copy），直接在多个闭包用即可；`SecondTick` 只在 running 分支内 `use_context` 读取，done/failed 行零 tick 订阅（性能契约）。

- [ ] **Step 5: 编译验证 + Commit**

Run: `cargo test -p aleph-panel --lib tool_card`
Expected: PASS（既有 tool_card 纯逻辑测试全绿；组件部分编译过即可）

```bash
git add interfaces/webchat/src/state/run_clock.rs interfaces/webchat/src/state/mod.rs interfaces/webchat/src/app.rs interfaces/webchat/src/components/tool_card.rs
git commit -m "panel: slim tool card to single-line row with status glyphs and live elapsed"
```

---

### Task 5: `messages.rs` 渲染重写 — 叙述流 + 工具行 + 探索块（+ i18n）

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/messages.rs`、`interfaces/webchat/locales/zh.json`、`interfaces/webchat/locales/en.json`
- Test: messages.rs 内嵌 tests（删旧加新）

**Interfaces:**
- Consumes: Task 2 行模型、Task 3 `explore_entries`、Task 4 瘦身 ToolCard、`ChatState::strip_is_open/toggle_strip`（key 改用 explore group key）
- Produces: 无对外新接口（渲染终端）

- [ ] **Step 1: i18n 词条**

`zh.json` 的 `"chat"` 段加（`en.json` 对应）：

```json
"explore_running": "探索中…",      // en: "Exploring…"
"explore_done": "探索了",          // en: "Explored"
"explore_items": "项",             // en: "items"
```

（探索块折叠摘要的大类计数标签复用现有 `tool_card.cat_read`/`cat_search`。）

- [ ] **Step 2: 删旧渲染**

- 删 `StepStrip` 组件（`messages.rs:856-935`）及 `MessageList` 中 `TimelineRow::StepStrip` match arm（含 Task 2 加的临时兜底 arm）。
- 删 `latest_step_tool`（`:423-431`）、`step_narration_head`（`:433-443`）与 `mod step_action_tests` 整个测试模块。
- 删 `MessageBubble` 的 `in_strip` prop 及其分支（`:467-471`、`bubble_style` 的 `in_strip` arm `:494-502`）。
- 删 `MessageBubble` 的 focused ring / dom id（`:525-546` 的 `msg_iteration`/`focused`/`bubble_class_reactive`/`bubble_dom_id`；气泡 class 退回静态 `bubble_class`，`id` 属性去掉）。`tool_calls_view`（`:548-568`）里 `iteration=it_for_cards` 传参与 `it_for_cards` 绑定一并删除（ToolCard 的 iteration prop 是 optional，Task 7 将整个移除该 prop）。`use crate::state::layout::WorkspaceState;` 若仍被 tool payload 查找使用则保留。

- [ ] **Step 3: 新增渲染组件**

`MessageList` 的 `<For>` match arms：

```rust
TimelineRow::Narration { message } => view! {
    <NarrationRow message=message />
}.into_any(),
TimelineRow::ToolLine { run_id, tool } => view! {
    <div class="px-1">
        <ToolCard run_id=run_id tool_id=tool.tool_id tool_name=tool.tool_name />
    </div>
}.into_any(),
TimelineRow::ExploreGroup { key, run_id, tools, completed } => view! {
    <ExploreGroupRow key_id=key run_id=run_id tools=tools completed=completed />
}.into_any(),
```

`NarrationRow`（无框直排，比最终回答淡一档）：

```rust
/// 中间轮叙述 — 无框直排的过程独白，永久留在对话流里。
#[component]
fn NarrationRow(message: ChatMessage) -> impl IntoView {
    let content = message.content.clone();
    let message_id = message.id.clone();
    let is_streaming = message.is_streaming;
    view! {
        <div class="px-1 py-0.5 text-sm text-text-secondary leading-relaxed aleph-step-narration">
            <TypewriterRenderer content=content message_id=message_id is_streaming=is_streaming />
        </div>
    }
}
```

`ExploreGroupRow`（头部 + 展开体；展开态存 `chat.strip_open`，key = group key；默认：运行中开、完成后收）：

```rust
/// 探索聚合块 — 连续只读工具塌缩为一个可展开块（codex Exploring 同源）。
/// 展开态按 group key 存 `ChatState::strip_open`（扛 per-token remount）。
#[component]
fn ExploreGroupRow(
    key_id: String,
    run_id: String,
    tools: Vec<crate::views::chat::state::ToolCallEntry>,
    completed: bool,
) -> impl IntoView {
    use crate::components::tool_card::{explore_entries, summarize_tools, tool_headline, ToolKind};
    let chat = expect_context::<ChatState>();
    let workspace = use_context::<WorkspaceState>();
    let i18n = use_i18n();

    let default_open = !completed;
    let open = {
        let k = key_id.clone();
        Memo::new(move |_| chat.strip_is_open(&k, default_open))
    };

    // 折叠头摘要：运行中 "🔍 探索中… N 项"；完成 "✓ 探索了 N 项（读取×3 · 搜索×1）"
    let n = tools.len();
    let counts = summarize_tools(
        &tools.iter().map(|t| (t.tool_id.clone(), t.tool_name.clone())).collect::<Vec<_>>(),
    )
    .into_iter()
    .map(|(k, c)| {
        let label = match k {
            ToolKind::FileRead => t_string!(i18n, tool_card.cat_read).to_string(),
            ToolKind::Search => t_string!(i18n, tool_card.cat_search).to_string(),
            _ => t_string!(i18n, tool_card.cat_tool).to_string(),
        };
        format!("{label}×{c}")
    })
    .collect::<Vec<_>>()
    .join(" · ");
    let header = move || if completed {
        format!("{} {} {}（{}）",
            t_string!(i18n, chat.explore_done), n,
            t_string!(i18n, chat.explore_items), counts.clone())
    } else {
        format!("{} {} {}",
            t_string!(i18n, chat.explore_running), n,
            t_string!(i18n, chat.explore_items))
    };

    // 展开体条目：headline 从 payload 现算（合并逻辑在纯函数里）。
    let entries = {
        let tools = tools.clone();
        let run = run_id.clone();
        Memo::new(move |_| {
            let items: Vec<(String, String, Option<String>)> = tools.iter().map(|t| {
                let kind = ToolKind::from_name(&t.tool_name);
                let payload = workspace.and_then(|w| w.get_tool_payload(&run, &t.tool_id));
                (t.tool_id.clone(), t.tool_name.clone(), tool_headline(kind, &payload))
            }).collect();
            explore_entries(&items)
        })
    };

    let k_for_toggle = key_id;
    let run_for_click = run_id;
    view! {
        <div class="my-0.5">
            <button
                type="button"
                class="w-full flex items-center gap-2 px-1 py-0.5 text-left text-sm
                       text-text-tertiary hover:text-text-secondary"
                on:click=move |_| chat.toggle_strip(&k_for_toggle, default_open)
            >
                {if completed {
                    view! { <span class="text-success shrink-0 text-[11px]">"✓"</span> }.into_any()
                } else {
                    view! { <span class="shrink-0 inline-block w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span> }.into_any()
                }}
                <span class="shrink-0">"🔍"</span>
                <span class="flex-1 min-w-0 truncate">{header}</span>
                <span class="shrink-0 text-[10px]">
                    {move || if open.get() { "▾" } else { "▸" }}
                </span>
            </button>
            <Show when=move || open.get()>
                <div class="pl-7 flex flex-col gap-0.5">
                    <For
                        each=move || entries.get()
                        key=|e| e.tool_ids.join(",")
                        children=move |e| {
                            let icon = crate::components::tool_card::tool_icon("", e.kind);
                            let first = e.tool_ids.first().cloned().unwrap_or_default();
                            let run = run_for_click.clone();
                            view! {
                                <button
                                    type="button"
                                    class="flex items-center gap-2 px-1 py-0.5 text-left text-xs
                                           text-text-tertiary hover:text-primary min-w-0"
                                    on:click=move |_| {
                                        if let Some(ws) = workspace {
                                            ws.select_tool(run.clone(), first.clone());
                                        }
                                    }
                                >
                                    <span class="shrink-0">{icon}</span>
                                    <span class="truncate">{e.label.clone()}</span>
                                </button>
                            }
                        }
                    />
                </div>
            </Show>
        </div>
    }
}
```

注意：`ws.select_tool` 到 Task 6 才存在——本任务先写 `ws.select_tool(...)` 调用，与 Task 6 同一 PR 序列内编译闭合；若需独立绿灯，本任务临时用 `let _ = (run.clone(), first.clone());` 占位并留 `// TODO(Task 6): select_tool` 是**不允许的**（No Placeholders）——因此**本任务与 Task 6 的 Step 3 存在编译依赖，执行顺序上先做 Task 6 Step 1-3（状态层），再回来做本任务**。（执行者按 Task 6 → Task 5 顺序做，提交顺序也如此。）

`ChatState.strip_open` 的 doc 注释（state.rs:405-412）改为「per explore-group expand override, keyed by group key」。

- [ ] **Step 4: 编译 + 既有测试**

Run: `cargo test -p aleph-panel --lib messages && cargo test -p aleph-panel --lib timeline`
Expected: PASS（`run_id_tests` 保留通过；step_action_tests 已删）

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/messages.rs interfaces/webchat/src/platform/wide/views/chat/state.rs interfaces/webchat/locales/zh.json interfaces/webchat/locales/en.json
git commit -m "panel: narration-led transcript with tool lines and explore groups"
```

---

### Task 6: `WorkspaceState` — `selected_tool`/跟随钉住，退役 step 交叉高亮

> **执行顺序注意**：本任务的 Step 1-3（状态层）先于 Task 5 执行（Task 5 依赖 `select_tool`）。

**Files:**
- Modify: `interfaces/webchat/src/state/layout.rs`
- Test: 同文件 tests

**Interfaces:**
- Produces（Task 5/7 消费）:

```rust
pub selected_tool: RwSignal<Option<(String, String)>>, // (run_id, tool_id)
pub pinned: RwSignal<bool>,
pub fn follow_tool(&self, run_id: &str, tool_id: &str);   // 未钉住时跟随（events.rs 直播）
pub fn select_tool(&self, run_id: impl Into<String>, tool_id: impl Into<String>); // 用户点选：选中+钉住+开 Split
pub fn end_follow(&self);                                  // run 结束解钉（保留选中显示）
```

- 删除: `focused_step`、`current_iteration`、`focus_step`、`is_step_focused`、`set_current_iteration`、`reveal_tool`（及其全部单测）。

- [ ] **Step 1: 写失败测试**

替换 layout.rs tests 里 `focus_step_sets_focus_and_opens_split` / `is_step_focused_discriminates_run_and_iteration` / `set_current_iteration_tracks_active_turn` / `reset_clears_focus_and_current_iteration` / `reveal_tool_*` 五组，新增：

```rust
#[test]
fn follow_tool_tracks_latest_unless_pinned() {
    let owner = Owner::new();
    owner.set();
    let ws = test_ws(LayoutMode::Split);
    ws.follow_tool("r1", "t1");
    assert_eq!(ws.selected_tool.get_untracked(),
               Some(("r1".to_string(), "t1".to_string())));
    // 用户点选 → 钉住
    ws.select_tool("r1", "t2");
    assert!(ws.pinned.get_untracked());
    // 钉住后直播跟随不再覆盖
    ws.follow_tool("r1", "t3");
    assert_eq!(ws.selected_tool.get_untracked(),
               Some(("r1".to_string(), "t2".to_string())));
    // run 结束解钉，选中保留
    ws.end_follow();
    assert!(!ws.pinned.get_untracked());
    assert!(ws.selected_tool.get_untracked().is_some());
    // 解钉后恢复跟随
    ws.follow_tool("r2", "t9");
    assert_eq!(ws.selected_tool.get_untracked(),
               Some(("r2".to_string(), "t9".to_string())));
}

#[test]
fn select_tool_opens_split() {
    let owner = Owner::new();
    owner.set();
    let ws = test_ws(LayoutMode::ChatOnly);
    ws.select_tool("r1", "t1");
    assert_eq!(ws.mode.get_untracked(), LayoutMode::Split);
}

#[test]
fn reset_clears_selection_and_pin() {
    let owner = Owner::new();
    owner.set();
    let ws = test_ws(LayoutMode::Split);
    ws.select_tool("r1", "t1");
    ws.reset();
    assert!(ws.selected_tool.get_untracked().is_none());
    assert!(!ws.pinned.get_untracked());
}
```

`test_ws` fixture 同步：删 `focused_step`/`current_iteration` 字段，加 `selected_tool: RwSignal::new(None), pinned: RwSignal::new(false)`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p aleph-panel --lib layout`
Expected: FAIL（编译错误——新字段/方法不存在）

- [ ] **Step 3: 实现**

struct 字段替换（`layout.rs:109-117`）：

```rust
/// 详情查看器当前选中的工具 `(run_id, tool_id)`。直播时由
/// `follow_tool` 跟随最新开始的工具；用户点选（`select_tool`）后钉住。
pub selected_tool: RwSignal<Option<(String, String)>>,
/// 用户是否钉住了选中（钉住时直播跟随不覆盖）。run 结束解除。
pub pinned: RwSignal<bool>,
```

方法（替换 `focus_step`/`is_step_focused`/`set_current_iteration`/`reveal_tool`，`new()`/`reset()` 同步）：

```rust
/// 直播跟随：未钉住时把详情面切到最新开始的工具（R5 — 工作台感）。
pub fn follow_tool(&self, run_id: &str, tool_id: &str) {
    if !self.pinned.get_untracked() {
        self.selected_tool
            .set(Some((run_id.to_string(), tool_id.to_string())));
    }
}

/// 用户点选：选中 + 钉住 + 确保 Split 打开（聊天侧任何"→ 详情"入口都走这里）。
pub fn select_tool(&self, run_id: impl Into<String>, tool_id: impl Into<String>) {
    self.selected_tool.set(Some((run_id.into(), tool_id.into())));
    self.pinned.set(true);
    if self.mode.get_untracked() != LayoutMode::Split {
        self.set_layout(LayoutMode::Split);
    }
}

/// run 完成/出错：解除钉住（选中保留，详情面继续显示最后的工具）。
pub fn end_follow(&self) {
    self.pinned.set(false);
}
```

`reset()` 加 `self.selected_tool.set(None); self.pinned.set(false);`，删 `focused_step`/`current_iteration` 两行。模块头注释第 11-13 行同步。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p aleph-panel --lib layout`
Expected: PASS（此时 workspace_panel/messages/view/events/chat_sidebar 编译失败为预期，Task 5/7 闭合；先不提交，与 Task 5 或 Task 7 一起编译闭合后提交）

- [ ] **Step 5: Commit（与 Task 5 联合提交点之后单独提交本文件亦可）**

```bash
git add interfaces/webchat/src/state/layout.rs
git commit -m "panel: workspace selection model with live-follow and pin"
```

---

### Task 7: 工作区详情查看器 + 全链路连线

**Files:**
- Modify: `interfaces/webchat/src/components/workspace_panel.rs`（删 ActivityTimeline/StepCard/timeline_groups/StepGroup + tests，新 ToolDetailView）
- Modify: `interfaces/webchat/src/components/tool_card.rs`（溢出行 `on_overflow` 改走 `select_tool`；`iteration` prop 删除；`render_body` 改 `pub(crate)`）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/events.rs`（follow/unfollow 连线）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/view.rs:106-125`（删 focused_step 滚动 Effect）
- Modify: `interfaces/webchat/src/components/chat_sidebar.rs:215-218`（session 加载清理新信号）
- Modify: `interfaces/webchat/locales/zh.json` / `en.json`（详情空态词条）
- Test: workspace_panel.rs（旧 timeline_groups tests 删除）

**Interfaces:**
- Consumes: Task 6 `selected_tool`/`follow_tool`/`select_tool`/`end_follow`、Task 4 瘦身 ToolCard、`render_body`/`tool_headline`/`tool_icon`/`ToolKind`/`ToolSurface`
- Produces: 无对外新接口

- [ ] **Step 1: i18n**

`zh.json` `"common"` 段加：

```json
"workspace_detail_empty": "点击左侧工具行查看完整参数与结果",   // en: "Click a tool row on the left to inspect its full args and result"
```

- [ ] **Step 2: tool_card.rs 连线改造**

- `render_body` 前加 `pub(crate)`（ToolDetailView 复用）。
- `ToolCard` 删 `iteration` prop；`on_overflow` 改为：

```rust
let on_overflow = move || {
    if let Some(ws) = workspace {
        ws.select_tool(run_for_overflow.clone(), tid_for_overflow.clone());
    }
};
```

（`default_open` 变量若因此只剩 expand 用途，保持不动。）调用点同步删 `iteration=` 传参：`messages.rs`（Task 5 的 ToolLine 已不传）、`workspace_panel.rs`（StepCard 整体删除）。

- [ ] **Step 3: workspace_panel.rs 重写单代理路径**

删 `StepGroup`/`timeline_groups`/`ActivityTimeline`/`StepCard`/`WorkspaceEmptyHero` 的 step 语义与文件尾 tests 模块（两个 timeline_groups 测试）。`WorkspacePanel` 单代理 fallback 改为（滚动跟随逻辑删除——详情是单视图不需要 stick-to-bottom）：

```rust
fallback=move || view! {
    <div class="flex-1 overflow-y-auto px-4 pb-3 aleph-content-top">
        <ToolDetailView />
    </div>
    <FilesDrawer />
}
```

新组件：

```rust
/// 详情查看器 — 右栏主体：当前选中工具的完整 args/result/diff（不封顶）。
/// 选中来源：直播跟随（events.rs → follow_tool）或用户点选（select_tool）。
#[component]
fn ToolDetailView() -> impl IntoView {
    use crate::components::tool_card::{
        render_body, tool_headline, tool_icon, ToolKind, ToolSurface,
    };
    let workspace = expect_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();
    let i18n = use_i18n();

    move || match workspace.selected_tool.get() {
        None => view! {
            <div class="h-full flex flex-col items-center justify-center
                        text-center text-text-tertiary gap-3 py-12 px-6">
                <p class="text-sm font-medium text-text-secondary">{t!(i18n, common.workspace_pane)}</p>
                <p class="text-xs max-w-[28ch] leading-relaxed">
                    {t!(i18n, common.workspace_detail_empty)}
                </p>
            </div>
        }.into_any(),
        Some((run_id, tool_id)) => {
            // 名字/状态从 transcript 反查；payload 从捕获表取。
            let entry = chat.messages.with(|msgs| {
                msgs.iter()
                    .flat_map(|m| m.tool_calls.iter())
                    .find(|t| t.tool_id == tool_id)
                    .cloned()
            });
            let tool_name = entry.as_ref().map(|t| t.tool_name.clone()).unwrap_or_default();
            let status = entry.as_ref().map(|t| t.status.clone()).unwrap_or_default();
            let duration = entry.as_ref().and_then(|t| t.duration_ms);
            let kind = ToolKind::from_name(&tool_name);
            let payload = workspace.get_tool_payload(&run_id, &tool_id);
            let headline = tool_headline(kind, &payload).unwrap_or_else(|| tool_name.clone());
            let icon = tool_icon(&tool_name, kind);
            let status_view = match status.as_str() {
                "running" => view! { <span class="inline-block w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span> }.into_any(),
                "failed" => view! { <span class="text-danger text-xs">"✗"</span> }.into_any(),
                _ => view! { <span class="text-success text-xs">"✓"</span> }.into_any(),
            };
            view! {
                <div class="flex flex-col gap-2">
                    <div class="flex items-center gap-2 pb-2 border-b border-border/60">
                        <span class="text-base shrink-0">{icon}</span>
                        <span class="flex-1 min-w-0 truncate text-sm text-text-primary font-medium">
                            {headline}
                        </span>
                        {status_view}
                        {duration.map(|d| view! {
                            <span class="text-[10px] font-mono text-text-tertiary">
                                {crate::state::run_clock::fmt_elapsed(d as i64)}
                            </span>
                        })}
                    </div>
                    {render_body(kind, &payload, ToolSurface::Detail, String::new(), || {})}
                </div>
            }.into_any()
        }
    }
}
```

（`render_body` Detail surface 不产生溢出行，`detail_label`/`on_overflow` 传空即可。）

- [ ] **Step 4: 全链路连线**

- `events.rs:46` 区（`tool_call_started` arm，`record_tool_args` 之后）加：`workspace.follow_tool(run_id, tool_id);`
- `events.rs:128`：删 `workspace.set_current_iteration(run_id, iteration);`
- `events.rs:451` 与 `:486`：`workspace.current_iteration.set(None);` → `workspace.end_follow();`
- `view.rs:106-125`：整段 focused_step 滚动 Effect 删除（含其注释；`workspace` 绑定若再无读者一并清）。
- `chat_sidebar.rs:215-218`：

```rust
if let Some(ws) = workspace {
    ws.unseen_activity.set(0);
    ws.selected_tool.set(None);
    ws.pinned.set(false);
}
```

- [ ] **Step 5: 全量 panel 测试**

Run: `cargo test -p aleph-panel --lib`
Expected: 全 PASS（本任务闭合了 Task 5/6 遗留的编译面）

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/components/workspace_panel.rs interfaces/webchat/src/components/tool_card.rs interfaces/webchat/src/components/chat_sidebar.rs interfaces/webchat/src/platform/wide/views/chat/events.rs interfaces/webchat/src/platform/wide/views/chat/view.rs interfaces/webchat/locales/zh.json interfaces/webchat/locales/en.json
git commit -m "panel: workspace pane becomes tool detail viewer with live follow"
```

---

### Task 8: prompt 微调 — 叙述分组 / trivial 免叙述 / 推进感

**Files:**
- Modify: `src/thinker/layers/multi_step_conduct.rs:85-103`（Section 2 文案）+ 同文件 tests

**Interfaces:**
- Consumes/Produces: 无接口变化，仅 prompt 文本。门控（`PromptMode::Full` + 非 SilentReply + `input.context` 存在）**不动**。

- [ ] **Step 1: 更新断言（先改测试）**

在现有 `assert!(out.contains("## Narrate Your Progress"))` 的测试里追加：

```rust
assert!(out.contains("Group logically related actions"));
assert!(out.contains("Skip the preamble for a single trivial read"));
```

Run: `cargo test -p alephcore --lib multi_step_conduct`
Expected: FAIL（新文案未写入）

- [ ] **Step 2: 改文案**

`inject` 的 Section 2 三条 bullet 替换为（codex preamble 三规则融入，保持 8-12 词口径与 recap 条不变）：

```rust
output.push_str(
    "- Before an action or a batch of related actions, post a one-line preamble \
     (roughly 8-12 words) of what you're about to do. Group logically related \
     actions under ONE preamble — don't narrate every single tool call.\n",
);
output.push_str(
    "- Skip the preamble for a single trivial read (opening one file, one quick \
     lookup); narrate the batch it belongs to instead.\n",
);
output.push_str(
    "- Connect each preamble to what came before — e.g. \"Config found — now \
     wiring the new field.\" — so progress reads as one thread.\n",
);
output.push_str(
    "- After finishing each plan step, post a brief recap, e.g. \"Done: the data \
     model is in place.\", so the user can follow along.\n\n",
);
```

- [ ] **Step 3: 跑测试确认通过**

Run: `cargo test -p alephcore --lib multi_step_conduct`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/thinker/layers/multi_step_conduct.rs
git commit -m "thinker: preamble grouping and trivial-read exemption in narrate guidance"
```

---

### Task 9: 收尾验证 — 全量测试 + WASM 构建 + 视觉走查 + 文档锚点

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md:448-453`（§6.1 锚点与打磨话术更新）

- [ ] **Step 1: 全量 panel + 定向 core 测试**

Run: `cargo test -p aleph-panel --lib && cargo test -p alephcore --lib multi_step_conduct`
Expected: 全 PASS

- [ ] **Step 2: WASM 构建 + 重编 server**

Run: `just wasm && cargo build --bin aleph-server 2>&1 | tail -5`
Expected: 编译成功（Panel↔Daemon 嵌入链：不重编 binary 看不到改动）

- [ ] **Step 3: 视觉走查（Puppeteer headless，参考全局 memory `reference_panel_testing`）**

启动 dev server 后走查五个场景，逐项截图确认：
1. 发起一个多步工具任务：叙述文本无框流式出现，工具是单行条目。
2. 连读多文件：探索块实时聚合（▾ 开、条目合并），完成后自动折叠成 `✓ 探索了 N 项` 一行，叙述文本仍在。
3. 长跑工具（如 sleep 10 的 bash）：8s 后行尾出现递增耗时。
4. 点击工具行"→ 详情"/探索条目：右栏打开详情（完整 result，无 8 行封顶），钉住后直播不再跳走；run 结束后新 run 的工具恢复跟随。
5. ChatOnly 模式 + 切换会话：无残留选中、红点徽章行为不变、历史会话的过程行正确渲染（叙述常驻、探索块折叠）。

发现问题就地修复（每修一处跑对应 `--lib <filter>` 测试）。

- [ ] **Step 4: 更新 FEATURE_LOCATOR §6.1**

改写 448-453 行：锚点加 `state/run_clock.rs`、`ExploreGroup`/`ToolDetailView`/`selected_tool`；职责描述改为「叙述主线 transcript + 探索聚合 + 详情查看器」；打磨话术更新（expanded_events 契约不变；新增「探索块展开态在 `ChatState::strip_open` 按 group key」「详情选中/钉住在 `WorkspaceState::selected_tool`/`pinned`」）。

- [ ] **Step 5: Final commit**

```bash
git add docs/reference/FEATURE_LOCATOR.md
git commit -m "docs: refresh feature locator for narration-led streaming echo (§6.1)"
```
