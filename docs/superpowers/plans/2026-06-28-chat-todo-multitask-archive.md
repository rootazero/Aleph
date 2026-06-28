# 单聊 Todo 多任务覆盖 + 沉入对话流 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 同一 session 多次任务分解时,固定 Todo 面板只显示单一活动计划(覆盖),完成/被替换的计划沉成一条紧凑胶囊进对话流;live 与 replay 走同一投影,刷新后仍在。

**Architecture:** 纯 panel 侧(`interfaces/webchat`,零 core 改动)。退场统一为 `ChatState::archive_active_plan(gate)`:把当前 `plan` 压成一条 `plan_archive` 胶囊消息推入 `messages`,再清空 `plan`。挂在两类 live 与 replay 共用的调用点——`start_assistant_message()`(下一轮触发,仅沉已完成)与 `events.rs` 的 scratchpad `set_plan`/`clear` 分支(覆盖/收尾触发,沉有进展的)。胶囊作为普通 `ChatMessage` 随 `messages` 进 `SessionSnapshot`(切 tab 保留),并由 replay 重放同一串 scratchpad 事件确定性重建(刷新保留)。

**Tech Stack:** Rust + Leptos (WASM panel),crate `aleph-panel`。serde for `ChatMessage`/`PlanView` 持久化。

## Global Constraints

- **零 core 改动**:所有改动限 `interfaces/webchat/src/`。无新协议、无新依赖、无 `SessionSnapshot` 新字段。
- **红线**:R4(面板纯渲染模型产出的快照信号)、R7/R10(不新增 harness 逻辑、不做"任务是否完成/该结束"的确定性判断——`complete`/`set_plan`/`clear` 均为模型显式信号;"下一轮只沉已完成、进行中不沉"即把判断留给模型)。
- **构建策略(项目铁律:极度节制 cargo)**:实现者**只写测试 + 代码,不跑 cargo**。每个 Task 列出测试命令与预期,由**控制器在任务评审检查点批量验证**——host 单测 `cargo test -p aleph-panel --lib <name>`,WASM 构建 `just wasm`。不要在每一步真跑 cargo。
- **提交规范**:English,`<scope>: <description>`;无 attribution(用户全局禁用)。单分支 main 直接开发。
- **dist 产物**:`interfaces/webchat/dist/aleph_panel*.{js,wasm}` 由控制器 `just wasm` 统一重生,实现者勿手改。

---

### Task 1: `plan.rs` — serde 化 + `has_activity()` + 胶囊摘要

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/plan.rs`
- Test: 同文件 `#[cfg(test)] mod tests`(已存在,追加用例)

**Interfaces:**
- Produces:
  - `PlanView`/`PlanItemView`/`PlanItemStatusView` 新增 `serde::Serialize + serde::Deserialize`(供 Task 2 的 `ChatMessage.plan_archive` 进 `SessionSnapshot`)。
  - `PlanView::has_activity(&self) -> bool`(供 Task 2/3 的 `Activity` 门控)。
  - `PlanView::archive_summary(&self) -> (&'static str, String)` 返回 `(glyph, label)`:完成 `("✓", "任务完成 · {done}/{total}")`,未完成 `("◗", "未完成 · {done}/{total}")`(供 Task 4 渲染)。

- [ ] **Step 1: 给三个类型加 serde derive**

`PlanItemStatusView`(第 8 行附近)derive 行改为:
```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PlanItemStatusView {
    Pending,
    InProgress,
    Completed,
}
```
`PlanItemView`(第 15 行附近):
```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlanItemView {
    pub text: String,
    pub status: PlanItemStatusView,
}
```
`PlanView`(第 21 行附近):
```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlanView {
    pub objective: Option<String>,
    pub items: Vec<PlanItemView>,
    pub complete: bool,
}
```

- [ ] **Step 2: 写失败测试(has_activity + archive_summary)**

在 `mod tests` 末尾追加:
```rust
fn pv(items: &[(&str, PlanItemStatusView)], complete: bool) -> PlanView {
    PlanView {
        objective: Some("Ship".into()),
        items: items
            .iter()
            .map(|(t, s)| PlanItemView { text: (*t).into(), status: s.clone() })
            .collect(),
        complete,
    }
}

#[test]
fn has_activity_true_when_worked_or_complete() {
    use PlanItemStatusView::*;
    // pristine: all pending, not complete → no activity
    assert!(!pv(&[("a", Pending), ("b", Pending)], false).has_activity());
    // one in-progress → activity
    assert!(pv(&[("a", InProgress), ("b", Pending)], false).has_activity());
    // one done → activity
    assert!(pv(&[("a", Completed), ("b", Pending)], false).has_activity());
    // complete flag → activity even if items somehow read pending
    assert!(pv(&[("a", Pending)], true).has_activity());
    // empty plan, not complete → no activity
    assert!(!pv(&[], false).has_activity());
}

#[test]
fn archive_summary_glyph_and_label() {
    use PlanItemStatusView::*;
    let done = pv(&[("a", Completed), ("b", Completed)], true);
    assert_eq!(done.archive_summary(), ("✓", "任务完成 · 2/2".to_string()));
    let partial = pv(&[("a", Completed), ("b", Pending)], false);
    assert_eq!(partial.archive_summary(), ("◗", "未完成 · 1/2".to_string()));
}

#[test]
fn plan_view_roundtrips_serde() {
    let p = pv(&[("a", PlanItemStatusView::InProgress)], false);
    let s = serde_json::to_string(&p).unwrap();
    let back: PlanView = serde_json::from_str(&s).unwrap();
    assert_eq!(p, back);
}
```

- [ ] **Step 3: 运行确认失败(控制器批验)**

Run: `cargo test -p aleph-panel --lib plan::tests`
Expected: FAIL —`has_activity`/`archive_summary` 未定义,编译错误。

- [ ] **Step 4: 实现两个方法**

在 `impl PlanView { ... }`(`has_content` 之后)追加:
```rust
    /// `true` when the plan was actually worked on (≥1 done/in-progress) or is
    /// marked complete — the gate for archiving a superseded/cleared plan. A
    /// pristine just-set plan (all pending, not complete) returns `false`, so a
    /// quick re-`set_plan` refinement is silently replaced, not archived.
    #[must_use]
    pub fn has_activity(&self) -> bool {
        self.complete
            || self.items.iter().any(|i| {
                matches!(
                    i.status,
                    PlanItemStatusView::InProgress | PlanItemStatusView::Completed
                )
            })
    }

    /// Glyph + label for the sunk archive capsule: `("✓", "任务完成 · d/t")`
    /// when complete, else `("◗", "未完成 · d/t")`.
    #[must_use]
    pub fn archive_summary(&self) -> (&'static str, String) {
        let (glyph, word) = if self.complete { ("✓", "任务完成") } else { ("◗", "未完成") };
        (glyph, format!("{word} · {}/{}", self.done_count(), self.total()))
    }
```

- [ ] **Step 5: 运行确认通过(控制器批验)**

Run: `cargo test -p aleph-panel --lib plan::tests`
Expected: PASS(原有 3 用例 + 新增 3 用例全绿)。

- [ ] **Step 6: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/plan.rs
git commit -m "panel: make PlanView serde + add has_activity/archive_summary"
```

---

### Task 2: `state.rs` — `plan_archive` 字段 + `archive_active_plan()` + 下一轮归档钩子

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/state.rs`
- Modify(仅补 `plan_archive: None` 字段,编译驱动):
  - `interfaces/webchat/src/platform/wide/views/chat/team_events.rs:47`
  - `interfaces/webchat/src/platform/wide/views/chat/timeline.rs`(6 个 test helper:`311 329 347 365 431 458`)
  - `interfaces/webchat/src/platform/wide/views/chat/transcript.rs:89`(test helper)
  - `interfaces/webchat/src/components/chat_sidebar.rs:70` 与 `:166`
  - `interfaces/webchat/src/components/workspace_panel.rs:597`
- Test: `state.rs` 内 `#[cfg(test)] mod step_tests`(已存在,追加用例)

**Interfaces:**
- Consumes: `PlanView::has_activity()`(Task 1)。
- Produces:
  - `ChatMessage.plan_archive: Option<PlanView>`(`Some` ⇒ Task 4 渲染胶囊)。
  - `pub enum ArchiveGate { Activity, Completed }`。
  - `ChatState::archive_active_plan(&self, gate: ArchiveGate)`(供 Task 3 在 `set_plan`/`clear` 前调用)。

- [ ] **Step 1: 加字段到 `ChatMessage`**

在 `ChatMessage`(第 166 行附近)`agent_id` 字段之后追加:
```rust
    /// Sunk archive of a finished/superseded scratchpad plan. `Some` ⇒ this
    /// message renders as a compact "completed task" capsule instead of normal
    /// text. Reconstructed identically by live projection and `replay_run`
    /// (both drive the same archive call sites), so it survives a tab swap (via
    /// `messages` in `SessionSnapshot`) and a full reload (via replay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_archive: Option<super::plan::PlanView>,
```

- [ ] **Step 2: 补全所有 `ChatMessage { .. }` 字面量的新字段**

在每处 `ChatMessage { .. }` struct 字面量里加 `plan_archive: None,`。本文件三处生产构造:`push_user_message`(~540)、`start_assistant_message`(~563)、`begin_step` 内 `msgs.push`(~612)。**跨文件**字面量同样补:`team_events.rs:47`、`timeline.rs` 六个 helper、`transcript.rs:89`、`chat_sidebar.rs:70 / :166`、`workspace_panel.rs:597`。
(`src/api/chat.rs` 的 `ChatMessage` 是另一个 wire 类型,**不要动**。)

- [ ] **Step 3: 写失败测试(archive_active_plan + 下一轮钩子)**

在 `mod step_tests` 末尾追加:
```rust
    use super::super::plan::{PlanItemStatusView, PlanItemView, PlanView};

    fn plan(items: &[(&str, PlanItemStatusView)], complete: bool) -> PlanView {
        PlanView {
            objective: Some("Obj".into()),
            items: items
                .iter()
                .map(|(t, s)| PlanItemView { text: (*t).into(), status: s.clone() })
                .collect(),
            complete,
        }
    }

    fn archive_count(chat: &ChatState) -> usize {
        chat.messages.with(|m| m.iter().filter(|x| x.plan_archive.is_some()).count())
    }

    #[test]
    fn archive_activity_sinks_worked_plan_and_clears_slot() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.plan.set(Some(plan(&[("a", PlanItemStatusView::Completed)], false)));
        chat.archive_active_plan(ArchiveGate::Activity);
        assert_eq!(archive_count(&chat), 1, "worked plan sinks one capsule");
        assert!(chat.plan.get_untracked().is_none(), "slot cleared after archive");
    }

    #[test]
    fn archive_activity_skips_pristine_plan() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.plan.set(Some(plan(&[("a", PlanItemStatusView::Pending)], false)));
        chat.archive_active_plan(ArchiveGate::Activity);
        assert_eq!(archive_count(&chat), 0, "pristine plan is not archived");
        assert!(chat.plan.get_untracked().is_some(), "slot left for overwrite");
    }

    #[test]
    fn archive_completed_gate_ignores_incomplete() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.plan.set(Some(plan(&[("a", PlanItemStatusView::InProgress)], false)));
        chat.archive_active_plan(ArchiveGate::Completed);
        assert_eq!(archive_count(&chat), 0, "in-progress plan not sunk on Completed gate");
        assert!(chat.plan.get_untracked().is_some());
    }

    #[test]
    fn start_assistant_message_sinks_completed_plan() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.plan.set(Some(plan(&[("a", PlanItemStatusView::Completed)], true)));
        chat.start_assistant_message("r2");
        assert_eq!(archive_count(&chat), 1, "completed plan sinks at next run start");
        assert!(chat.plan.get_untracked().is_none());
    }

    #[test]
    fn start_assistant_message_keeps_incomplete_plan() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.plan.set(Some(plan(&[("a", PlanItemStatusView::InProgress)], false)));
        chat.start_assistant_message("r2");
        assert_eq!(archive_count(&chat), 0, "in-progress plan stays in the sticky slot");
        assert!(chat.plan.get_untracked().is_some());
    }
```

- [ ] **Step 4: 运行确认失败(控制器批验)**

Run: `cargo test -p aleph-panel --lib state::step_tests`
Expected: FAIL —`ArchiveGate`/`archive_active_plan` 未定义。

- [ ] **Step 5: 实现 `ArchiveGate` + `archive_active_plan` + 钩子**

在 `state.rs` 顶部 `use` 区(`use super::plan::{PlanUpdate, PlanView};` 一行)改为:
```rust
use super::plan::{PlanUpdate, PlanView};
```
（已含 `PlanView`，无需改。）在 `ChatState` 的 `impl` 块内（`apply_plan_update` 附近）追加:
```rust
    /// Which sink trigger is calling — decides the archive gate.
    pub fn archive_active_plan(&self, gate: ArchiveGate) {
        let Some(p) = self.plan.get_untracked() else { return };
        let should = match gate {
            ArchiveGate::Activity => p.has_activity(),
            ArchiveGate::Completed => p.complete,
        };
        if !should {
            return; // leave the slot for the caller to overwrite/hide
        }
        let seq = self.next_msg_id.get_untracked();
        self.next_msg_id.set(seq + 1);
        self.messages.update(|msgs| {
            msgs.push(ChatMessage {
                id: format!("plan-archive-{seq}"),
                role: "assistant".into(),
                content: String::new(),
                tool_calls: vec![],
                is_streaming: false,
                is_intermediate: false,
                error: None,
                model_info: None,
                is_final: false,
                text_finalized: false,
                timestamp: Some(super::timeline::now_millis()),
                iteration: None,
                agent_id: None,
                plan_archive: Some(p),
            });
        });
        self.plan.set(None);
    }
```
并在 `ChatState` 之外(文件内合适位置,如 `ChatPhase` 附近)新增:
```rust
/// Which sink trigger is archiving the active plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveGate {
    /// New `set_plan` / `clear` — archive only a worked-on plan (`has_activity`).
    Activity,
    /// Next-turn start — archive only an already-complete plan.
    Completed,
}
```
最后在 `start_assistant_message` **顶部第一行**(`let id = ...` 之前)插入:
```rust
        // Next-turn sink: a finished plan retires into the conversation flow
        // when the next run begins. Both live (`run_accepted`) and replay
        // (`replay_run`) call this, so the capsule reconstructs identically.
        self.archive_active_plan(ArchiveGate::Completed);
```

- [ ] **Step 6: 运行确认通过(控制器批验)**

Run: `cargo test -p aleph-panel --lib state::step_tests`
Expected: PASS(原有 step_tests + 5 新用例全绿;`begin_step_*` 等不受影响)。

- [ ] **Step 7: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/state.rs \
        interfaces/webchat/src/platform/wide/views/chat/team_events.rs \
        interfaces/webchat/src/platform/wide/views/chat/timeline.rs \
        interfaces/webchat/src/platform/wide/views/chat/transcript.rs \
        interfaces/webchat/src/components/chat_sidebar.rs \
        interfaces/webchat/src/components/workspace_panel.rs
git commit -m "panel: ChatMessage.plan_archive + archive_active_plan + next-turn sink hook"
```

---

### Task 3: `events.rs` — 覆盖/收尾归档接线 + replay 一致性测试

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/events.rs:84-105`(scratchpad 投影分支)
- Test: 同文件 `#[cfg(test)] mod projection_tests`(已存在,追加用例)

**Interfaces:**
- Consumes: `ChatState::archive_active_plan(ArchiveGate::Activity)`(Task 2)、`scratchpad_plan_update`(现有)。
- Produces: 无新公共 API(纯接线)。

- [ ] **Step 1: 写失败测试(覆盖留痕 / 微调静默 / 同源 replay)**

在 `mod projection_tests` 末尾追加(沿用现有 `scratchpad_string_encoded_output_projects_plan_to_panel` 的 wire 形态构造):
```rust
    fn scratchpad_event(action: &str, items: &[(&str, &str)]) -> serde_json::Value {
        // Build the real wire shape: Success.output is a JSON-encoded STRING
        // whose `snapshot` carries the plan.
        let items_json: Vec<serde_json::Value> = items
            .iter()
            .map(|(status, text)| json!({ "status": status, "text": text }))
            .collect();
        let complete = !items.is_empty()
            && items.iter().all(|(s, _)| *s == "completed");
        let snapshot = json!({ "complete": complete, "objective": "Obj", "items": items_json });
        let output = serde_json::to_string(&json!({
            "success": true, "message": "ok", "snapshot": snapshot
        }))
        .unwrap();
        json!({
            "kind": "tool_call_completed", "iteration": 1,
            "call": { "tool_id": "s1", "tool_name": "scratchpad", "duration_ms": 1,
                      "input": { "action": action } },
            "result": { "Success": { "output": output } }
        })
    }

    fn archive_count(chat: &ChatState) -> usize {
        chat.messages.with(|m| m.iter().filter(|x| x.plan_archive.is_some()).count())
    }

    #[test]
    fn set_plan_supersede_archives_worked_prior() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        chat.start_assistant_message("r1");
        // plan A, then start an item (activity), then a fresh set_plan B
        apply_trace_event(chat, ws, "r1", &scratchpad_event("set_plan", &[("in_progress", "a")]));
        apply_trace_event(chat, ws, "r1", &scratchpad_event("set_plan", &[("pending", "b")]));
        assert_eq!(archive_count(&chat), 1, "worked prior plan A sinks");
        let plan = chat.plan.get_untracked().expect("new plan B shown");
        assert_eq!(plan.items[0].text, "b");
    }

    #[test]
    fn set_plan_supersede_skips_pristine_prior() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        chat.start_assistant_message("r1");
        // plan A pristine (all pending), immediately replaced → silent
        apply_trace_event(chat, ws, "r1", &scratchpad_event("set_plan", &[("pending", "a")]));
        apply_trace_event(chat, ws, "r1", &scratchpad_event("set_plan", &[("pending", "b")]));
        assert_eq!(archive_count(&chat), 0, "pristine prior A is silently replaced");
        assert_eq!(chat.plan.get_untracked().unwrap().items[0].text, "b");
    }

    #[test]
    fn clear_archives_completed_then_hides() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        chat.start_assistant_message("r1");
        apply_trace_event(chat, ws, "r1", &scratchpad_event("set_plan", &[("completed", "a")]));
        apply_trace_event(chat, ws, "r1", &scratchpad_event("clear", &[]));
        assert_eq!(archive_count(&chat), 1, "completed plan sinks on clear");
        assert!(chat.plan.get_untracked().is_none(), "panel hidden after clear");
    }

    #[test]
    fn start_item_update_does_not_archive() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        let ws = WorkspaceState::new();
        chat.start_assistant_message("r1");
        apply_trace_event(chat, ws, "r1", &scratchpad_event("set_plan", &[("pending", "a"), ("pending", "b")]));
        // a same-plan update (start_item) must NOT sink a capsule
        apply_trace_event(chat, ws, "r1", &scratchpad_event("start_item", &[("in_progress", "a"), ("pending", "b")]));
        assert_eq!(archive_count(&chat), 0, "in-place update is not a supersede");
    }
```

- [ ] **Step 2: 运行确认失败(控制器批验)**

Run: `cargo test -p aleph-panel --lib events::projection_tests`
Expected: FAIL —尚未接线,`set_plan_supersede_*` 等断言不满足(archive_count==0 期望 1)。

- [ ] **Step 3: 接线(在 apply_plan_update 之前归档)**

`events.rs` 中 scratchpad 分支(当前第 104 行 `chat.apply_plan_update(...)` 之前)插入:
```rust
                // Sink the prior plan before the new snapshot overwrites it.
                // Only a fresh decomposition (`set_plan`) or explicit teardown
                // (`clear`) supersedes — `start_item`/`complete_item`/
                // `set_objective` are in-place updates to the SAME plan and must
                // not archive. Gated on `has_activity` so a pristine refinement
                // is silently replaced.
                if action == "set_plan" || action == "clear" {
                    chat.archive_active_plan(super::state::ArchiveGate::Activity);
                }
                chat.apply_plan_update(super::plan::scratchpad_plan_update(action, snapshot));
```
(即在原 `chat.apply_plan_update(...)` 上方加 4 行 `if`;`ArchiveGate` 经 `super::state::` 引用,无需新 `use`。)

- [ ] **Step 4: 运行确认通过(控制器批验)**

Run: `cargo test -p aleph-panel --lib events::projection_tests`
Expected: PASS(原有 projection_tests + 4 新用例全绿;`scratchpad_string_encoded_output_projects_plan_to_panel` 仍绿——首次 set_plan 无前序计划,不归档)。

- [ ] **Step 5: 写 replay 一致性测试**

继续在 `mod projection_tests` 追加(验证 live 与 `replay_run` 对同一事件序列产出相同胶囊集合):
```rust
    #[test]
    fn replay_reconstructs_same_archive_capsules() {
        let owner = Owner::new();
        owner.set();
        // Live path
        let live = ChatState::new();
        let ws1 = WorkspaceState::new();
        live.start_assistant_message("r1");
        apply_trace_event(live, ws1, "r1", &scratchpad_event("set_plan", &[("completed", "a")]));
        live.start_assistant_message("r2"); // next-turn sink of completed A
        let live_caps = live.messages.with(|m| {
            m.iter().filter_map(|x| x.plan_archive.clone()).collect::<Vec<_>>()
        });

        // Replay path: same two runs reconstructed via replay_run
        let rep = ChatState::new();
        let ws2 = WorkspaceState::new();
        replay_run(rep, ws2, "r1", &[scratchpad_event("set_plan", &[("completed", "a")])], "done");
        replay_run(rep, ws2, "r2", &[], "next");
        let rep_caps = rep.messages.with(|m| {
            m.iter().filter_map(|x| x.plan_archive.clone()).collect::<Vec<_>>()
        });

        assert_eq!(live_caps.len(), 1, "live sinks one capsule");
        assert_eq!(rep_caps.len(), 1, "replay reconstructs the same one");
        assert_eq!(live_caps, rep_caps, "live and replay capsules are identical");
    }
```

- [ ] **Step 6: 运行确认通过(控制器批验)**

Run: `cargo test -p aleph-panel --lib events::projection_tests::replay_reconstructs_same_archive_capsules`
Expected: PASS。

- [ ] **Step 7: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/events.rs
git commit -m "panel: sink prior plan into chat on set_plan/clear (live + replay)"
```

---

### Task 4: 渲染 — 胶囊 cell + 完成态细条

**Files:**
- Create: `interfaces/webchat/src/platform/wide/views/chat/plan_archive_cell.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/chat/mod.rs`(注册 `mod plan_archive_cell; pub use ...`)
- Modify: `interfaces/webchat/src/platform/wide/views/chat/messages.rs:214-220`(`TimelineRow::Message` 分支按 `plan_archive` 切渲染)
- Modify: `interfaces/webchat/src/platform/wide/views/chat/todo_panel.rs:34-36`(完成态 header 文案)

**Interfaces:**
- Consumes: `ChatMessage.plan_archive`(Task 2)、`PlanView::archive_summary` / `PlanView::items` / `PlanItemStatusView`(Task 1)。
- Produces: `pub fn PlanArchiveCell(plan: PlanView) -> impl IntoView`(Leptos `#[component]`)。

- [ ] **Step 1: 新建胶囊组件**

`plan_archive_cell.rs`:
```rust
//! `PlanArchiveCell` — a sunk (completed/superseded) scratchpad plan rendered
//! as a compact, click-to-expand capsule in the conversation flow. Pure
//! presentation (R4): the data comes from `ChatMessage.plan_archive`, projected
//! by `events.rs` from the model's scratchpad signals.

use leptos::prelude::*;

use super::plan::{PlanItemStatusView, PlanView};

#[component]
pub fn PlanArchiveCell(plan: PlanView) -> impl IntoView {
    let expanded = RwSignal::new(false);
    let (glyph, label) = plan.archive_summary();
    let objective = plan.objective.clone().unwrap_or_default();
    let complete = plan.complete;
    let items = plan.items.clone();
    view! {
        <style>{ARCHIVE_CELL_CSS}</style>
        <div class="aleph-plan-cap" class:done=move || complete>
            <button class="aleph-plan-cap-head" on:click=move |_| expanded.update(|e| *e = !*e)>
                <span class="aleph-plan-cap-glyph">{glyph}</span>
                <span class="aleph-plan-cap-label">{label}</span>
                <span class="aleph-plan-cap-obj">{objective}</span>
                <span class="aleph-plan-cap-chev" class:open=move || expanded.get()>"▾"</span>
            </button>
            <Show when=move || expanded.get()>
                <ul class="aleph-plan-cap-rows">
                    <For
                        each=move || items.clone()
                        key=|it| (it.text.clone(), it.status.clone())
                        let:it
                    >
                        {
                            let (cls, mark) = match it.status {
                                PlanItemStatusView::Completed => ("done", "✓"),
                                PlanItemStatusView::InProgress => ("active", "◗"),
                                PlanItemStatusView::Pending => ("pending", "·"),
                            };
                            view! {
                                <li class=format!("aleph-plan-cap-row {cls}")>
                                    <span class="aleph-plan-cap-box">{mark}</span>
                                    <span class="aleph-plan-cap-txt">{it.text.clone()}</span>
                                </li>
                            }
                        }
                    </For>
                </ul>
            </Show>
        </div>
    }
}

const ARCHIVE_CELL_CSS: &str = r#"
.aleph-plan-cap{max-width:760px;margin:2px auto;border:1px solid var(--color-border);
  border-radius:12px;background:color-mix(in oklch,var(--color-surface-overlay) 88%,transparent);
  overflow:hidden;font-size:12.5px}
.aleph-plan-cap-head{display:flex;align-items:center;gap:8px;width:100%;padding:5px 12px;
  background:transparent;border:0;cursor:pointer;color:var(--color-text-secondary,#888);text-align:left}
.aleph-plan-cap-glyph{flex:0 0 auto;font-weight:700}
.aleph-plan-cap.done .aleph-plan-cap-glyph{color:var(--color-success)}
.aleph-plan-cap-label{flex:0 0 auto;font-weight:600;font-variant-numeric:tabular-nums}
.aleph-plan-cap-obj{flex:1 1 auto;min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;
  color:var(--color-text-primary);opacity:.75}
.aleph-plan-cap-chev{flex:0 0 auto;margin-left:auto;transition:transform .18s}
.aleph-plan-cap-chev.open{transform:rotate(180deg)}
.aleph-plan-cap-rows{list-style:none;margin:0;padding:2px 10px 8px}
.aleph-plan-cap-row{display:flex;align-items:flex-start;gap:8px;padding:3px 4px;line-height:1.4}
.aleph-plan-cap-row .aleph-plan-cap-box{flex:0 0 auto;width:15px;text-align:center}
.aleph-plan-cap-row.done .aleph-plan-cap-box{color:var(--color-success)}
.aleph-plan-cap-row.done .aleph-plan-cap-txt{text-decoration:line-through;opacity:.7}
"#;
```

- [ ] **Step 2: 注册模块**

`mod.rs` 在 `mod todo_panel;` 之后加 `mod plan_archive_cell;`,并在 `pub use todo_panel::TodoPanel;` 之后加:
```rust
pub use plan_archive_cell::PlanArchiveCell;
```

- [ ] **Step 3: 渲染分支(messages.rs)**

`messages.rs` 的 `<For>` 中 `TimelineRow::Message { message, clock }` 分支(第 218-220 行)改为:
```rust
                                    TimelineRow::Message { message, clock } => {
                                        if let Some(p) = message.plan_archive.clone() {
                                            view! { <PlanArchiveCell plan=p /> }.into_any()
                                        } else {
                                            view! { <MessageBubble message=message clock=clock /> }.into_any()
                                        }
                                    }
```
`MessageBubble` 是 `messages.rs` 本模块内的 `#[component] fn`(直接调用,无需导入)。`PlanArchiveCell` 在兄弟模块,经 `mod.rs` 的 `pub use` 暴露——在 `messages.rs` 顶部 `use` 区加一行:
```rust
use super::PlanArchiveCell;
```

- [ ] **Step 4: 完成态细条文案(todo_panel.rs)**

`todo_panel.rs` 第 33-36 行的 `header_label` 改为已完成时显示 `"✓ 已完成"`:
```rust
                let header_label = current
                    .clone()
                    .map(|c| format!("正在：{c}"))
                    .unwrap_or_else(|| if complete { "✓ 已完成".into() } else { "待开始".into() });
```
(其余进度环/百分比/折叠逻辑不变;完成态本就走 `class:done` 细条样式。)

- [ ] **Step 5: 验证(控制器:WASM 构建 + 视觉)**

Run: `just wasm`
Expected: 构建成功(无类型/借用错误)。胶囊与细条为视觉产物,运行时验证在 Task 完成后的 QA(见下方"运行时 QA")。
> 说明:Task 4 的纯逻辑(`archive_summary` 文案、`has_activity`)已在 Task 1 单测覆盖;组件渲染无 host 单测,靠 `just wasm` 编译门 + 截图 QA。

- [ ] **Step 6: 提交**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/plan_archive_cell.rs \
        interfaces/webchat/src/platform/wide/views/chat/mod.rs \
        interfaces/webchat/src/platform/wide/views/chat/messages.rs \
        interfaces/webchat/src/platform/wide/views/chat/todo_panel.rs
git commit -m "panel: render sunk plan capsule + completed sticky thin-bar"
```

---

## 运行时 QA(控制器/用户,实现完成后)

走 [[feedback-ios-panel-test-via-full-macos-app]]:`just shell-dev` 重编内置 core 服当前 dist → 单聊验证:
1. 让模型做多步任务 → 固定面板进度环推进 → 全勾后收成「✓ 已完成 N/N」细条仍在原位。
2. 发下一条消息(或让模型开新 `set_plan`)→ 上一份沉成对话流里的「✓ 任务完成 · N/N ▾」胶囊,点击展开看清单;固定面板换成新计划或隐藏。
3. 中途切换任务(未完成被新 set_plan 覆盖)→ 沉成灰「◗ 未完成 · d/t」胶囊。
4. 刷新/切走再切回会话 → 胶囊仍在(replay 重建)。

## Self-Review(对照 spec)

- **§4 三条触发**:set_plan 覆盖→Task 3;clear→Task 3;下一轮→Task 2 `start_assistant_message` 钩子。✅
- **§4 去噪(微调静默 / 下一轮只沉已完成 / 进行中不沉)**:`has_activity` 门控(Task 1/3)+ `Completed` 门控(Task 2)。✅
- **§5 持久化(live+replay 同源)**:归档挂在 `start_assistant_message` 与 `apply_trace_event`,replay 一致性测试 Task 3 Step 5。✅
- **§6 改动清单**:plan.rs(T1)/state.rs(T2)/events.rs(T3)/todo_panel.rs+胶囊组件+messages.rs(T4)逐项有任务。✅
- **§7 测试矩阵**:覆盖-有进展/微调静默/clear/完成→细条→下一轮沉/进行中不沉/replay 一致/serde 兼容——分散在 T1-T3 单测。✅
- **No-placeholder / 类型一致**:`has_activity`/`archive_summary`/`ArchiveGate`/`archive_active_plan`/`plan_archive`/`PlanArchiveCell` 在定义任务与消费任务间命名一致。✅
- **R4/R7/R10 零 core**:全部 `interfaces/webchat`,无 harness 判断。✅
