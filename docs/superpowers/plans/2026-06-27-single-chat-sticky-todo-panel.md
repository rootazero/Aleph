# 单聊 Chat 置顶实时 Todo 面板 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 agent 通过 `scratchpad` 工具分解的任务清单，以结构化形式送到单聊 Panel 渲染为一个置顶、原地更新、完成即打勾的实时 todo 面板。

**Architecture:** scratchpad 三态生命周期 + 收尾门控已存在；本计划 ① 让 `ScratchpadOutput` 携带结构化 `snapshot`（搭现有 `tool_call_completed` 事件顺风车，零新协议变体、零 harness emit 点）② Panel 端纯函数投影 snapshot 进 `ChatState.plan` 信号 ③ 新 `TodoPanel` 组件（进度环卡片，默认折叠）渲染并打勾。附带两个硬化：单 in_progress 不变量、自动 per-chat project_id。

**Tech Stack:** Rust (alephcore, tokio/serde) · Leptos/WASM (aleph-panel) · 设计 token OKLCH（`interfaces/webchat/styles/tailwind.css`）。

## Global Constraints

- **范围**：仅单聊（single-chat）Panel。团队聊天、CLI 富渲染、三原语统一重构均不在本计划内（YAGNI / R3 / P6）。
- **红线**：计划由 LLM 调工具产生（R7/R8）；Panel 纯 I/O 渲染（R4）；零 harness emit 点、零新 `LoopTraceEvent`/`AgentTraceEvent` 变体（R10）；UI 在 Leptos（R2）。
- **构建策略（重要，源自项目纪律）**：实现者**不主动跑 cargo**（系统负担）。每个 core 任务照常**写测试 + 写实现**，但 `cargo test`/`cargo check` 由**控制器在任务收尾批量执行一次**（`cargo test -p alephcore --lib <name>` 定向 / 至多一次 `cargo check -p alephcore --lib`）。Panel 任务由控制器跑 `just wasm`。计划里的 "Run:" 步骤 = 控制器执行的验证。
- **提交规范**：English `<scope>: <description>`；全局禁用 attribution（不加 Co-Authored-By）。单分支：直接提交 main。
- **状态命名**：DTO/视图三态序列化为 `"pending" | "in_progress" | "completed"`（与 `PlanItemStatus` doc 注释意图一致；原枚举 `Pending/InProgress/Done` 不改派生）。
- **权威运行时门**：Task 5 后由用户走 macOS 完整 App + iOS-sim 双端流程实测（重编本地 core 服新 dist）。

---

## File Structure

| 文件 | 职责 | 动作 |
|------|------|------|
| `src/builtin_tools/scratchpad.rs` | scratchpad 工具：新增 `PlanSnapshotDto`/映射、`progress_parts`、输出携带 snapshot、自动 project_id | Modify |
| `src/memory/scratchpad/manager.rs` | `set_item_status` 维护单 in_progress 不变量 | Modify |
| `interfaces/webchat/src/platform/wide/views/chat/plan.rs` | 纯函数：scratchpad 结果 → `PlanView`/`PlanUpdate` 投影 + 视图模型 | Create |
| `interfaces/webchat/src/platform/wide/views/chat/state.rs` | `ChatState` 新增 `plan` 信号 + `apply_plan_update` | Modify |
| `interfaces/webchat/src/platform/wide/views/chat/events.rs` | `tool_call_completed` 分支接 scratchpad 投影 | Modify |
| `interfaces/webchat/src/platform/wide/views/chat/todo_panel.rs` | `TodoPanel` 组件（进度环卡片 + 打勾动画 + 折叠） | Create |
| `interfaces/webchat/src/platform/wide/views/chat/view.rs` | 挂载 `<TodoPanel />` | Modify |
| `interfaces/webchat/src/platform/wide/views/chat/mod.rs` | 声明 `mod plan; mod todo_panel;` | Modify |

---

## Task 1: Core — `PlanSnapshotDto` 与 `ScratchpadOutput` 携带结构化快照

**Files:**
- Modify: `src/builtin_tools/scratchpad.rs`（`ScratchpadOutput` ~67–74；`progress_echo` ~125–137；call() 各 arm ~210–300）

**Interfaces:**
- Consumes: `crate::memory::scratchpad::{ScratchpadSnapshot, PlanItem, PlanItemStatus}`（已存在；`ScratchpadSnapshot { objective: Option<String>, items: Vec<PlanItem> }`，`PlanItem { text, status }`，`PlanItemStatus { Pending, InProgress, Done }`，`is_objective_complete()`）。
- Produces: `pub struct PlanSnapshotDto { objective: Option<String>, items: Vec<PlanItemDto>, complete: bool }`，序列化进 `ScratchpadOutput.snapshot`（Panel 经 `result["Success"]["output"]["snapshot"]` 读取）。

- [ ] **Step 1: 写失败测试**（加到 `scratchpad.rs` 的 `#[cfg(test)] mod tests`）

```rust
#[test]
fn plan_snapshot_dto_maps_three_states_and_completion() {
    use crate::memory::scratchpad::{PlanItem, PlanItemStatus, ScratchpadSnapshot};
    let snap = ScratchpadSnapshot {
        objective: Some("Ship auth".into()),
        items: vec![
            PlanItem { text: "Design".into(), status: PlanItemStatus::Done },
            PlanItem { text: "Build".into(), status: PlanItemStatus::InProgress },
            PlanItem { text: "Test".into(), status: PlanItemStatus::Pending },
        ],
    };
    let dto = PlanSnapshotDto::from(&snap);
    assert_eq!(dto.objective.as_deref(), Some("Ship auth"));
    assert_eq!(dto.items.len(), 3);
    assert_eq!(dto.complete, false); // not all done
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["items"][0]["status"], "completed");
    assert_eq!(json["items"][1]["status"], "in_progress");
    assert_eq!(json["items"][2]["status"], "pending");
}

#[test]
fn plan_snapshot_dto_complete_when_all_done() {
    use crate::memory::scratchpad::{PlanItem, PlanItemStatus, ScratchpadSnapshot};
    let snap = ScratchpadSnapshot {
        objective: Some("X".into()),
        items: vec![PlanItem { text: "a".into(), status: PlanItemStatus::Done }],
    };
    assert!(PlanSnapshotDto::from(&snap).complete);
}
```

- [ ] **Step 2: Run（控制器）— 确认 FAIL**

Run: `cargo test -p alephcore --lib plan_snapshot_dto`
Expected: FAIL（`PlanSnapshotDto` 未定义）

- [ ] **Step 3: 实现 DTO + 映射**（加到 `scratchpad.rs`，紧接 `ScratchpadArgs`/`ScratchpadOutput` 区域之上）

```rust
use crate::memory::scratchpad::{PlanItemStatus, ScratchpadSnapshot};

/// Serde-friendly mirror of `PlanItemStatus` (which derives no serde).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanItemStatusDto {
    Pending,
    InProgress,
    Completed,
}

impl From<PlanItemStatus> for PlanItemStatusDto {
    fn from(s: PlanItemStatus) -> Self {
        match s {
            PlanItemStatus::Pending => Self::Pending,
            PlanItemStatus::InProgress => Self::InProgress,
            PlanItemStatus::Done => Self::Completed,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanItemDto {
    pub text: String,
    pub status: PlanItemStatusDto,
}

/// Structured snapshot of the scratchpad plan, attached to `ScratchpadOutput`
/// so the Panel can render a live Todo widget (rides the existing
/// `tool_call_completed` event; no new protocol variant — R4/R10).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanSnapshotDto {
    pub objective: Option<String>,
    pub items: Vec<PlanItemDto>,
    pub complete: bool,
}

impl From<&ScratchpadSnapshot> for PlanSnapshotDto {
    fn from(s: &ScratchpadSnapshot) -> Self {
        Self {
            objective: s.objective.clone(),
            items: s
                .items
                .iter()
                .map(|i| PlanItemDto {
                    text: i.text.clone(),
                    status: i.status.into(),
                })
                .collect(),
            complete: s.is_objective_complete(),
        }
    }
}
```

- [ ] **Step 4: 给 `ScratchpadOutput` 加 `snapshot` 字段**

把 `ScratchpadOutput`（当前 ~67–74）改为：

```rust
/// Output from the scratchpad tool.
#[derive(Debug, Clone, Serialize)]
pub struct ScratchpadOutput {
    /// Whether the operation succeeded
    pub success: bool,
    /// Human-readable result message
    pub message: String,
    /// Scratchpad content (returned for Read/Initialize)
    pub content: Option<String>,
    /// Structured plan snapshot for the Panel Todo widget (mutating actions only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<PlanSnapshotDto>,
}
```

- [ ] **Step 5: 用 `progress_parts` 替换 `progress_echo`（单次 snapshot 读，文本+DTO 同源不漂移）**

把现有 `progress_echo`（~118–137）整体替换为：

```rust
/// Read the scratchpad snapshot once and produce BOTH the model-facing
/// progress echo text and the Panel-facing structured DTO, so the two never
/// drift. Fail-soft: returns (None, None) on any read error.
async fn progress_parts(manager: &ScratchpadManager) -> (Option<String>, Option<PlanSnapshotDto>) {
    match manager.snapshot().await {
        Ok(s) => {
            let text = if s.is_objective_complete() {
                s.render_completion()
            } else {
                s.render_progress()
            };
            (Some(text), Some(PlanSnapshotDto::from(&s)))
        }
        Err(_) => (None, None),
    }
}
```

- [ ] **Step 6: 各 arm 携带 snapshot**

在 call() 的 4 个 mutating arm（`SetObjective` / `SetPlan` / `StartItem` / `CompleteItem`）里，把
`content: progress_echo(&manager).await,` 替换为先取 parts 再填两字段。例如 `SetPlan` arm：

```rust
ScratchpadAction::SetPlan => {
    let items = args.items.unwrap_or_default();
    let items_ref: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
    manager.set_plan(&items_ref).await?;
    let (content, snapshot) = progress_parts(&manager).await;
    Ok(ScratchpadOutput {
        success: true,
        message: format!("Plan set with {} items", items.len()),
        content,
        snapshot,
    })
}
```

`SetObjective` / `StartItem` / `CompleteItem` 同款改法（保留各自原 `message`，新增 `let (content, snapshot) = progress_parts(&manager).await;` 并填 `content, snapshot`）。

其余 4 个 arm（`Initialize` / `Read` / `AppendNote` / `Clear`）的每个 `ScratchpadOutput { ... }` 字面量补 `snapshot: None,`（`Clear` 的 `None` 是 Panel "隐藏面板" 的信号，见 Task 4）。共 5 处 `ScratchpadOutput`（Initialize 有两处：exists 分支与新建分支）需补 `snapshot: None,`。

- [ ] **Step 7: Run（控制器）— 确认 PASS**

Run: `cargo test -p alephcore --lib plan_snapshot_dto`
Expected: PASS（2 测试通过）

- [ ] **Step 8: Commit**

```bash
git add src/builtin_tools/scratchpad.rs
git commit -m "scratchpad: attach structured PlanSnapshotDto to tool output"
```

---

## Task 2: Core — 维护"至多一项 in_progress"不变量

**Files:**
- Modify: `src/memory/scratchpad/manager.rs`（`set_item_status`）

**Interfaces:**
- Consumes: 无新增（内部方法）。
- Produces: 行为变化 — `start_item(i)` 现会把任何**其它** `[~]` 项降级为 `[ ]`，保证快照至多一项 `InProgress`（Panel 永远只高亮一项 + `current()` 唯一）。

- [ ] **Step 1: 写失败测试**（加到 `manager.rs` 的 `#[cfg(test)] mod tests`）

```rust
#[tokio::test]
async fn start_item_demotes_previous_in_progress() {
    let tmp = std::env::temp_dir().join(format!("aleph-sp-inv-{}", std::process::id()));
    let mgr = ScratchpadManager::with_dir(tmp.clone(), "test");
    mgr.initialize(Some("obj")).await.unwrap();
    mgr.set_plan(&["a", "b", "c"]).await.unwrap();
    mgr.start_item(0).await.unwrap();
    mgr.start_item(1).await.unwrap(); // must demote item 0
    let snap = mgr.snapshot().await.unwrap();
    let in_prog: Vec<usize> = snap
        .items
        .iter()
        .enumerate()
        .filter(|(_, it)| it.is_in_progress())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(in_prog, vec![1], "only the newest started item stays in progress");
    assert!(!snap.items[0].is_in_progress(), "previous in-progress demoted to pending");
    let _ = std::fs::remove_dir_all(&tmp);
}
```

> 注：`ScratchpadManager::with_dir(PathBuf, &str)` 已存在（manager.rs:223）。如该测试目录 helper 与本文件既有测试风格不一致，沿用本文件已有的 tempdir helper（保持一致）。

- [ ] **Step 2: Run（控制器）— 确认 FAIL**

Run: `cargo test -p alephcore --lib start_item_demotes_previous_in_progress`
Expected: FAIL（item 0 仍为 `[~]`，`in_prog == vec![0, 1]`）

- [ ] **Step 3: 实现降级**

把 `set_item_status`（现 body 见下）改为在写入新 `InProgress` 时把**其它** `[~]` 降级为 `[ ]`：

```rust
async fn set_item_status(
    &self,
    item_index: usize,
    status: PlanItemStatus,
) -> Result<(), AlephError> {
    let content = self.read().await?;
    let mut out = String::with_capacity(content.len());
    let mut count = 0usize;
    // Maintain the single-in-progress invariant: starting a new item reverts
    // any other active `[~]` item to pending.
    let demote_others = status == PlanItemStatus::InProgress;

    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let body = trimmed.trim_end_matches(['\n', '\r']);
        let is_item =
            body.starts_with("- [ ]") || body.starts_with("- [~]") || body.starts_with("- [x]");
        if is_item && body != "- [ ] ..." {
            let this = count;
            count += 1;
            let indent = &line[..line.len() - trimmed.len()];
            let after_marker = &trimmed[5..];
            if this == item_index {
                out.push_str(indent);
                out.push_str("- ");
                out.push_str(status.glyph());
                out.push_str(after_marker);
                continue;
            } else if demote_others && body.starts_with("- [~]") {
                out.push_str(indent);
                out.push_str("- ");
                out.push_str(PlanItemStatus::Pending.glyph());
                out.push_str(after_marker);
                continue;
            }
        }
        out.push_str(line);
    }

    let out = self.update_timestamp(out);
    self.write(&out).await
}
```

- [ ] **Step 4: Run（控制器）— 确认 PASS**

Run: `cargo test -p alephcore --lib start_item_demotes_previous_in_progress`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/memory/scratchpad/manager.rs
git commit -m "scratchpad: enforce single in-progress item on start_item"
```

---

## Task 3: Core — 自动 per-chat project_id（消除单聊取名摩擦）

**Files:**
- Modify: `src/builtin_tools/scratchpad.rs`（`ScratchpadArgs.project_id`；`call()` 头部；`DESCRIPTION`）

**Interfaces:**
- Consumes: `self.current_session_key().await`（已存在）、`scratchpad_registry::{set_active, clear}`（已存在）。
- Produces: `ScratchpadArgs.project_id` 变为 `Option<String>`；缺省由 `derive_default_project_id(session_key)` 派生合法 id；`call()` 内部统一用解析后的 `project_id`。

- [ ] **Step 1: 写失败测试**（`scratchpad.rs` 测试模块）

```rust
#[test]
fn derive_default_project_id_sanitizes_and_prefixes() {
    assert_eq!(derive_default_project_id("agent:abc/def 1"), "chat-agent-abc-def-1");
    assert_eq!(derive_default_project_id(""), "chat-default");
    assert_eq!(derive_default_project_id("///"), "chat-default");
    // result must pass the same path-safety rules call() enforces
    let id = derive_default_project_id("..\\evil");
    assert!(!id.contains("..") && !id.contains('/') && !id.contains('\\') && !id.starts_with('.'));
}
```

- [ ] **Step 2: Run（控制器）— 确认 FAIL**

Run: `cargo test -p alephcore --lib derive_default_project_id`
Expected: FAIL（函数未定义）

- [ ] **Step 3: 实现派生函数 + 改 args + 改 call() 头部**

(a) 在 `scratchpad.rs` 加纯函数：

```rust
/// Derive a filesystem-safe default scratchpad project id from the live
/// session key, for single-chat ad-hoc todos where the model omits
/// `project_id`. Keeps only `[A-Za-z0-9_-]`, prefixes `chat-` (so it never
/// starts with `.` and never collides with the path-traversal guard).
fn derive_default_project_id(session_key: &str) -> String {
    let slug: String = session_key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    // collapse runs of '-' and trim edges for a clean slug
    let mut collapsed = String::with_capacity(slug.len());
    let mut prev_dash = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash { collapsed.push('-'); }
            prev_dash = true;
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }
    let trimmed = collapsed.trim_matches('-');
    if trimmed.is_empty() {
        "chat-default".to_string()
    } else {
        format!("chat-{trimmed}")
    }
}
```

(b) 把 `ScratchpadArgs.project_id`（~24–26）改为可选：

```rust
    /// Project identifier (AI-assigned name). Optional — when omitted, the
    /// current chat session derives a default scratchpad, so single-chat
    /// todos work without naming a project. Pass an explicit id for a durable
    /// cross-session project.
    #[serde(default)]
    pub project_id: Option<String>,
```

(c) 改 `call()` 头部（当前：info! 日志 → 校验 args.project_id → session_key/registry → manager）。替换为先解析 `project_id`：

```rust
    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        // Resolve the effective project id: explicit, else derive from the
        // live chat session so single-chat todos need no project name.
        let session_key = self.current_session_key().await;
        let project_id = match args.project_id.clone() {
            Some(p) if !p.trim().is_empty() => p,
            _ => derive_default_project_id(&session_key),
        };

        info!(
            project_id = %project_id,
            action = %args.action,
            "Scratchpad operation requested"
        );

        // Validate project_id to prevent path traversal (applies to explicit ids;
        // derived ids are pre-sanitized and always pass).
        if project_id.contains("..")
            || project_id.contains('/')
            || project_id.contains('\\')
            || project_id.contains('\0')
            || project_id.starts_with('.')
        {
            return Err(crate::error::AlephError::tool(
                "Invalid project_id: must not contain path separators, '..', null bytes, or start with '.'".to_string(),
            ));
        }

        // Registry binding (unchanged semantics, now keyed on resolved id).
        if !session_key.is_empty() {
            match args.action {
                ScratchpadAction::Read => {}
                ScratchpadAction::Clear => scratchpad_registry::clear(&session_key),
                _ => scratchpad_registry::set_active(&session_key, &project_id),
            }
        }

        let manager = ScratchpadManager::new(&project_id, "tool");

        match args.action {
            // ... arms unchanged (they reference args.value/items/item_index) ...
```

> 注意：删除原先位于 call() 顶部的旧 `info!`、旧校验块、旧 `let session_key = self.current_session_key().await;` 与旧 registry 块、旧 `let manager = ScratchpadManager::new(&args.project_id, "tool");`，全部由上面新块取代（一次性替换，避免 `args.project_id` 残留——它已是 `Option`）。

(d) 更新 `DESCRIPTION` 末尾一句，说明 project_id 可选：在现有描述串尾部追加 `" The project_id is optional — omit it to use the current chat's scratchpad."`。

- [ ] **Step 4: Run（控制器）— 确认 PASS**

Run: `cargo test -p alephcore --lib derive_default_project_id`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/builtin_tools/scratchpad.rs
git commit -m "scratchpad: make project_id optional with per-chat default"
```

---

## Task 4: Panel — 投影纯函数 + `ChatState.plan` + events 连线

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/chat/plan.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/chat/state.rs`（`ChatState` 结构体 ~262–376；`new()` ~380+；`impl ChatState`）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/events.rs`（`tool_call_completed` 分支 ~57–84）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/mod.rs`（`mod plan;`）

**Interfaces:**
- Produces: `PlanView { objective: Option<String>, items: Vec<PlanItemView>, complete: bool }`（+ `done_count/total/percent/current_step/has_content`）、`PlanItemView { text, status: PlanItemStatusView }`、`PlanItemStatusView { Pending, InProgress, Completed }`、`PlanUpdate { Show(PlanView), Hide, NoChange }`、`pub fn scratchpad_plan_update(action: &str, snapshot: Option<&serde_json::Value>) -> PlanUpdate`。
- Consumes（Task 5）：`ChatState.plan: RwSignal<Option<PlanView>>`。
- Wire shape：scratchpad 工具结果在事件里为 `result["Success"]["output"]["snapshot"]`（`AgentTraceToolResult::Success { output }` 外部标签；`output` = `ScratchpadOutput` JSON）。

- [ ] **Step 1: 写失败测试**（新文件 `plan.rs` 底部 `#[cfg(test)] mod tests`）

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snap() -> serde_json::Value {
        json!({
            "objective": "Ship auth",
            "complete": false,
            "items": [
                {"text": "Design", "status": "completed"},
                {"text": "Build", "status": "in_progress"},
                {"text": "Test", "status": "pending"}
            ]
        })
    }

    #[test]
    fn set_plan_result_shows_plan() {
        let s = snap();
        match scratchpad_plan_update("set_plan", Some(&s)) {
            PlanUpdate::Show(v) => {
                assert_eq!(v.objective.as_deref(), Some("Ship auth"));
                assert_eq!(v.total(), 3);
                assert_eq!(v.done_count(), 1);
                assert_eq!(v.percent(), 33);
                assert_eq!(v.current_step(), Some("Build"));
            }
            other => panic!("expected Show, got {other:?}"),
        }
    }

    #[test]
    fn clear_hides_panel() {
        assert_eq!(scratchpad_plan_update("clear", None), PlanUpdate::Hide);
    }

    #[test]
    fn read_without_snapshot_is_no_change() {
        assert_eq!(scratchpad_plan_update("read", None), PlanUpdate::NoChange);
    }
}
```

- [ ] **Step 2: Run（控制器）— 确认 FAIL**

Run: `cargo test -p aleph-panel plan::tests`（或编译期失败：模块不存在）
Expected: FAIL（`plan` 模块/`scratchpad_plan_update` 未定义）

- [ ] **Step 3: 实现 `plan.rs`**

```rust
//! Pure projection of scratchpad tool results into the chat Todo panel state.
//!
//! Lives here (not in `state.rs`) so the projection logic is unit-testable
//! without a Leptos reactive runtime. `events.rs` is the only caller.

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanItemStatusView {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItemView {
    pub text: String,
    pub status: PlanItemStatusView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanView {
    pub objective: Option<String>,
    pub items: Vec<PlanItemView>,
    pub complete: bool,
}

impl PlanView {
    pub fn done_count(&self) -> usize {
        self.items
            .iter()
            .filter(|i| i.status == PlanItemStatusView::Completed)
            .count()
    }
    pub fn total(&self) -> usize {
        self.items.len()
    }
    pub fn percent(&self) -> u32 {
        if self.items.is_empty() {
            return 0;
        }
        ((self.done_count() as f64 / self.total() as f64) * 100.0).round() as u32
    }
    pub fn current_step(&self) -> Option<&str> {
        self.items
            .iter()
            .find(|i| i.status == PlanItemStatusView::InProgress)
            .map(|i| i.text.as_str())
    }
    /// The panel renders only when there is something to show.
    pub fn has_content(&self) -> bool {
        self.objective.is_some() || !self.items.is_empty()
    }
}

/// What the Todo panel should do in response to a completed scratchpad call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanUpdate {
    Show(PlanView),
    Hide,
    NoChange,
}

/// Pure projection. `action` = scratchpad call's `input.action`; `snapshot` =
/// `result["Success"]["output"]["snapshot"]` (None when absent). `clear` hides
/// the panel; a present snapshot shows it; everything else leaves it untouched.
pub fn scratchpad_plan_update(action: &str, snapshot: Option<&Value>) -> PlanUpdate {
    if action == "clear" {
        return PlanUpdate::Hide;
    }
    match snapshot.and_then(parse_plan_view) {
        Some(view) => PlanUpdate::Show(view),
        None => PlanUpdate::NoChange,
    }
}

fn parse_plan_view(snapshot: &Value) -> Option<PlanView> {
    let obj = snapshot.as_object()?;
    let objective = obj
        .get("objective")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let complete = obj.get("complete").and_then(|v| v.as_bool()).unwrap_or(false);
    let items = obj
        .get("items")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(parse_item).collect())
        .unwrap_or_default();
    Some(PlanView { objective, items, complete })
}

fn parse_item(v: &Value) -> Option<PlanItemView> {
    let o = v.as_object()?;
    let text = o.get("text")?.as_str()?.to_string();
    let status = match o.get("status").and_then(|s| s.as_str()) {
        Some("in_progress") => PlanItemStatusView::InProgress,
        Some("completed") => PlanItemStatusView::Completed,
        _ => PlanItemStatusView::Pending,
    };
    Some(PlanItemView { text, status })
}
```

- [ ] **Step 4: `mod.rs` 声明模块**

在 `chat/mod.rs` 加（与既有 `pub mod state;` 等并列）：

```rust
pub mod plan;
```

- [ ] **Step 5: `state.rs` 加 `plan` 信号 + `apply_plan_update`**

(a) 顶部 import：`use super::plan::{PlanUpdate, PlanView};`
(b) `ChatState` 结构体（`#[derive(Clone, Copy)]`）新增字段（放在 `team_*` 字段附近）：

```rust
    /// Active single-chat task plan (scratchpad-driven Todo widget). `None`
    /// hides the panel. Projected by `events.rs` via `scratchpad_plan_update`.
    pub plan: RwSignal<Option<PlanView>>,
```

(c) `new()` 初始化（与其它字段并列）：`plan: RwSignal::new(None),`
(d) `impl ChatState` 加方法：

```rust
    /// Apply a projected plan update to the Todo-panel signal.
    pub fn apply_plan_update(&self, update: PlanUpdate) {
        match update {
            PlanUpdate::Show(v) => self.plan.set(Some(v)),
            PlanUpdate::Hide => self.plan.set(None),
            PlanUpdate::NoChange => {}
        }
    }
```

- [ ] **Step 6: `events.rs` 接线**

在 `tool_call_completed` 分支末尾（line ~83 `workspace.record_tool_result(...)` 之后、该 arm 闭合 `}` 之前）追加：

```rust
            // Project scratchpad plan snapshots into the sticky Todo panel.
            if tool_name == "scratchpad" {
                let action = call
                    .and_then(|c| c.get("input"))
                    .and_then(|i| i.get("action"))
                    .and_then(|a| a.as_str())
                    .unwrap_or("");
                let snapshot = result
                    .get("Success")
                    .and_then(|s| s.get("output"))
                    .and_then(|o| o.get("snapshot"));
                chat.apply_plan_update(super::plan::scratchpad_plan_update(action, snapshot));
            }
```

- [ ] **Step 7: Run（控制器）— 确认 PASS + 编译**

Run: `cargo test -p aleph-panel plan::tests` 然后 `just wasm`
Expected: 3 测试 PASS；`just wasm` 编译通过

- [ ] **Step 8: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/plan.rs \
        interfaces/webchat/src/platform/wide/views/chat/state.rs \
        interfaces/webchat/src/platform/wide/views/chat/events.rs \
        interfaces/webchat/src/platform/wide/views/chat/mod.rs
git commit -m "panel: project scratchpad snapshot into chat plan state"
```

---

## Task 5: Panel — `TodoPanel` 进度环卡片组件 + 挂载

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/chat/todo_panel.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/chat/mod.rs`（`mod todo_panel;`）
- Modify: `interfaces/webchat/src/platform/wide/views/chat/view.rs`（挂载 + import）

**Interfaces:**
- Consumes: `expect_context::<ChatState>()`、`super::plan::{PlanView, PlanItemStatusView}`、`chat.plan` 信号。
- Produces: `#[component] pub fn TodoPanel() -> impl IntoView`。

> **测试说明**：Leptos 组件的视觉/响应行为无法在无浏览器运行时下单元测试；本任务的纯逻辑（百分比、当前步骤、可见性）已在 Task 4 的 `PlanView` helper 单测覆盖。本任务的验证 = `just wasm` 编译通过 + Task 5 末尾的用户运行时 QA。动画与配色用组件内嵌 `<style>`（自包含，不依赖 Tailwind 构建管线）。

- [ ] **Step 1: 创建 `todo_panel.rs`**

```rust
//! `TodoPanel` — single-chat sticky Todo widget (progress-ring card).
//!
//! Renders `ChatState.plan` as a collapsed progress-ring header (default) that
//! expands into a checklist; each completed item draws a ✓ and flashes. Hidden
//! when there is no active plan. Pure presentation (R4) — the plan is produced
//! by the LLM via the `scratchpad` tool (R7/R8).

use leptos::prelude::*;

use super::plan::{PlanItemStatusView, PlanView};
use super::state::ChatState;

#[component]
pub fn TodoPanel() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let expanded = RwSignal::new(false);

    let visible = move || chat.plan.with(|p| p.as_ref().is_some_and(PlanView::has_content));

    view! {
        <style>{TODO_PANEL_CSS}</style>
        <Show when=visible>
            {move || {
                let plan = chat.plan.get().expect("visible implies Some");
                let pct = plan.percent();
                let done = plan.done_count();
                let total = plan.total();
                let current = plan.current_step().map(str::to_string);
                let complete = plan.complete;
                let ring_style = format!(
                    "background: conic-gradient(var(--color-success) {pct}%, var(--color-border-subtle) 0);"
                );
                let header_label = current
                    .clone()
                    .map(|c| format!("正在：{c}"))
                    .unwrap_or_else(|| if complete { "已完成".into() } else { "待开始".into() });
                view! {
                    <div class="aleph-todo-wrap" class:done=move || complete>
                        // ── header (always visible) — click to toggle ──
                        <button
                            class="aleph-todo-head"
                            on:click=move |_| expanded.update(|e| *e = !*e)
                        >
                            <span class="aleph-todo-ring" style=ring_style>
                                <span class="aleph-todo-ring-inner">{move || format!("{pct}%")}</span>
                            </span>
                            <span class="aleph-todo-meta">
                                <b>{move || format!("任务计划 · {done}/{total}")}</b>
                                <small>{header_label}</small>
                            </span>
                            <span class="aleph-todo-chev" class:open=move || expanded.get()>"▾"</span>
                        </button>
                        // ── checklist (expanded only) ──
                        <Show when=move || expanded.get()>
                            <ul class="aleph-todo-rows">
                                <For
                                    each=move || chat.plan.get().map(|p| p.items).unwrap_or_default()
                                    key=|it| (it.text.clone(), it.status.clone())
                                    let:it
                                >
                                    {
                                        let (cls, glyph) = match it.status {
                                            PlanItemStatusView::Completed => ("done", "✓"),
                                            PlanItemStatusView::InProgress => ("active", ""),
                                            PlanItemStatusView::Pending => ("pending", ""),
                                        };
                                        view! {
                                            <li class=format!("aleph-todo-row {cls}")>
                                                <span class="aleph-todo-box">{glyph}</span>
                                                <span class="aleph-todo-txt">{it.text.clone()}</span>
                                            </li>
                                        }
                                    }
                                </For>
                            </ul>
                        </Show>
                    </div>
                }
            }}
        </Show>
    }
}

/// Self-contained styles (OKLCH design tokens; check-draw + flash animations).
const TODO_PANEL_CSS: &str = r#"
.aleph-todo-wrap{margin:6px auto 0;max-width:760px;border:1px solid var(--color-border);
  border-radius:14px;background:color-mix(in oklch,var(--color-surface-overlay) 92%,transparent);
  backdrop-filter:blur(8px);overflow:hidden;font-size:13px}
.aleph-todo-head{display:flex;align-items:center;gap:12px;width:100%;padding:9px 13px;
  background:transparent;border:0;cursor:pointer;color:var(--color-text-primary);text-align:left}
.aleph-todo-ring{flex:0 0 auto;width:36px;height:36px;border-radius:50%;display:grid;place-items:center}
.aleph-todo-ring-inner{width:27px;height:27px;border-radius:50%;background:var(--color-surface-raised);
  display:grid;place-items:center;font-size:10px;font-weight:700;font-variant-numeric:tabular-nums}
.aleph-todo-meta{display:flex;flex-direction:column;gap:1px;min-width:0}
.aleph-todo-meta b{font-size:13px}
.aleph-todo-meta small{font-size:11.5px;color:var(--color-text-secondary,oklch(0.55 0.01 310));
  white-space:nowrap;overflow:hidden;text-overflow:ellipsis;max-width:52ch}
.aleph-todo-chev{margin-left:auto;font-size:11px;transition:transform .18s;color:var(--color-text-secondary,#888)}
.aleph-todo-chev.open{transform:rotate(180deg)}
.aleph-todo-rows{list-style:none;margin:0;padding:4px 8px 8px}
.aleph-todo-row{display:flex;align-items:flex-start;gap:10px;padding:6px 8px;border-radius:9px;line-height:1.45}
.aleph-todo-box{flex:0 0 auto;width:17px;height:17px;border-radius:6px;border:1.6px solid var(--color-border);
  display:grid;place-items:center;margin-top:1px;font-size:11px;color:#fff}
.aleph-todo-row.done .aleph-todo-box{background:var(--color-success);border-color:var(--color-success);
  animation:aleph-todo-draw .4s ease-out}
.aleph-todo-row.done{animation:aleph-todo-flash 1.1s ease-out}
.aleph-todo-row.done .aleph-todo-txt{color:var(--color-text-secondary,#888);text-decoration:line-through}
.aleph-todo-row.active{background:var(--color-primary-subtle)}
.aleph-todo-row.active .aleph-todo-box{border-color:var(--color-primary)}
.aleph-todo-row.active .aleph-todo-box::after{content:"";width:9px;height:9px;border-radius:3px;
  background:var(--color-primary);animation:aleph-todo-pulse 1.2s ease-in-out infinite}
.aleph-todo-row.active .aleph-todo-txt{font-weight:600}
@keyframes aleph-todo-draw{from{transform:scale(.5);opacity:.3}to{transform:scale(1);opacity:1}}
@keyframes aleph-todo-flash{0%{background:var(--color-success-subtle)}100%{background:transparent}}
@keyframes aleph-todo-pulse{0%,100%{opacity:.4;transform:scale(.7)}50%{opacity:1;transform:scale(1)}}
"#;
```

> 注：上面用到的 token 均已核对存在于 `interfaces/webchat/styles/tailwind.css`（light+dark 两套）：`--color-success` / `--color-success-subtle` / `--color-primary` / `--color-primary-subtle` / `--color-border` / `--color-border-subtle` / `--color-surface-overlay` / `--color-surface-raised` / `--color-text-primary` / `--color-text-secondary`。**该项目无 `--color-bg`**，面板背景用 `--color-surface-overlay`、环心用 `--color-surface-raised`（已在上方 CSS 落实）。`--color-text-secondary` 已存在，保留的 fallback 仅作冗余防御。

- [ ] **Step 2: `mod.rs` 声明组件模块**

`chat/mod.rs` 加：

```rust
mod todo_panel;
pub use todo_panel::TodoPanel;
```

- [ ] **Step 3: `view.rs` 挂载**

(a) 顶部 import 区加：`use super::todo_panel::TodoPanel;`（或经 mod.rs re-export `use super::TodoPanel;`，与既有 import 风格对齐）。
(b) 在 overlap 容器内、SessionTabs overlay（line ~208）之后插入置顶面板：

```rust
                    // Session tab strip overlay …
                    <div class="absolute inset-x-0 top-0 z-10"><SessionTabs /></div>
                    // Single-chat sticky Todo panel — below the tab strip,
                    // above the message flow. Hidden when no active plan.
                    <div class="absolute inset-x-0 top-0 z-[11] px-3 pt-9 pointer-events-none">
                        <div class="pointer-events-auto"><TodoPanel /></div>
                    </div>
```

> `pt-9` 给顶部 SessionTabs 让位；`pointer-events-none` 外层 + `pointer-events-auto` 内层 = 折叠态不挡消息点击，仅面板自身可交互（与 TeamParticipants 同款手法）。如 SessionTabs 高度不同，实现时用目视微调 `pt-*`（运行时 QA 一并校准）。

- [ ] **Step 4: Run（控制器）— 编译**

Run: `just wasm`
Expected: 编译通过，dist 重建

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/todo_panel.rs \
        interfaces/webchat/src/platform/wide/views/chat/mod.rs \
        interfaces/webchat/src/platform/wide/views/chat/view.rs \
        interfaces/webchat/dist
git commit -m "panel: add sticky Todo panel widget to single chat"
```

- [ ] **Step 6: 用户运行时 QA（权威门）**

按用户既定流程：`just shell-build`（或重编本地 core 服新 dist）→ macOS 完整 App + iOS-sim 双端连同一本地 core。验收：
1. 模型 `scratchpad(set_plan=…)` → 顶部进度环卡片（折叠态）出现。
2. `start_item` → 当前步骤高亮 + 头部"正在：…"更新。
3. `complete_item` → 该项 ✓ 画入 + 绿闪 + 删除线，环进度推进。
4. 全部完成 / `clear` → 面板隐藏。
5. 无活跃计划时面板不可见；点头部可折叠/展开。

---

## Self-Review

**Spec coverage：**
- §4 Core snapshot → Task 1 ✅
- §5 Panel（state/events/component）→ Task 4 + Task 5 ✅
- §6 自动 project_id → Task 3 ✅
- §7 硬化：单 in_progress → Task 2 ✅；空计划/隐藏 → `has_content()` + `clear`→Hide（Task 4/5）✅；完成 banner → `complete` 标志 + 环 100%（Task 1/5）✅；veto 文本回显 → 已存在（events.rs:134，无需改）✅
- §8 范围边界：团队/CLI/三原语统一 → 明确不做 ✅
- §9 测试：DTO 映射（T1）、单 in_progress（T2）、派生 id（T3）、投影纯函数（T4）、运行时 QA（T5）✅

**Placeholder scan：** 无 TBD/TODO；每个 code step 给出完整代码；唯一两处"现场核对"是 token 名 `--color-bg` 与 `pt-*` 偏移，均明确标注为运行时校准点（非占位）。

**Type consistency：** `PlanSnapshotDto`（core，serde `pending/in_progress/completed`）↔ Panel `parse_item` 读同名串 ✅；`scratchpad_plan_update(action, snapshot)` 签名在 T4 定义、T6 events.rs 同签名调用 ✅；`ChatState.plan: RwSignal<Option<PlanView>>` 在 T4 state.rs 定义、T5 组件 `chat.plan.get()` 消费 ✅；`apply_plan_update` 在 T4 定义、events.rs 调用 ✅。
