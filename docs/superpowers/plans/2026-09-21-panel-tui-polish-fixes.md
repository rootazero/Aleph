# Panel & TUI Polish (Phase 2/3/4 — Tier-1 + Tier-2 修复) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the 11 Tier-1 bugs/wirings + 13 Tier-2 guardrail tests identified in Phase 1 audit (Plan 1), all within L1 scope (bug fixes + wiring + tests; no rewrites, no new public surfaces, no wire protocol changes).

**Architecture:** Sequential tasks in 3 phases — Phase 2 (Tier-1 fixes, user-visible), Phase 3 (Tier-2加固 + Playwright e2e), Phase 4 (docs sync). Each fix is a separate commit with TDD discipline: failing test → minimal impl → green → commit.

**Tech Stack:** Rust (alephcore, webchat/aleph-panel, tui/aleph-tui), Playwright e2e (Phase 3 only).

**Spec:** `docs/superpowers/specs/2026-09-21-panel-tui-polish-design.md`

**Audit source:** `docs/superpowers/plans/phase1-audit/gap-analysis.md` (approved 11 Tier-1 + 13 Tier-2 = 24 items)

---

## Global Constraints

- **Worktree only**: `/home/zou/data/workspace/Aleph-panel-tui-polish` (branch `panel-tui-polish`); never touch `main` (worktree at `/home/zou/data/workspace/Aleph/`)
- **Memory limits** (<16GB machine):
  - Always: `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1`
  - For alephcore lib test: only run pre-compiled binary if recompilation kills
- **Process management**: `pkill -f "target/(release|debug)/aleph-server"` between phases
- **Commit format**: `<scope>: <description>` (English). Examples: `panel: fix attachment chip For key`, `tui: retry subscribe_runtime_agents on failure`
- **Style**: rustfmt (4-space indent, 100 char width) + clippy (`-D warnings`)
- **TDD discipline**: Each fix: write failing test → run → see RED → implement minimal code → run → see GREEN → commit
- **No production-code changes to**: `shared/protocol/`, wire schema types, new public surface modules
- **Reference**: `docs/reference/FEATURE_LOCATOR.md` canonical status table; only modify entries that change status in this round

**CLAUDE.md compliance notes (2026-09-21 self-check)**:
- ✅ All R1–R10 architectural redlines respected (no platform API in src, Leptos-only UI logic, no new heavy deps, harness untouched)
- ✅ All P1–P8 design principles respected (YAGNI scope, local fixes, defensive guards only)
- ✅ All Do NOT introduce list respected (no 2nd async runtime / vector DB / VT / CDP)
- ⚠️ **Criterion 19 (One widening)** applies to T1.9 (adding `model_pin` to TUI `SessionKnobs` is a widening). Before Task A4, grep all sites counting "4 knobs" (`slash::SessionKnob::ALL`, status bar segment count, etc.) and update them in lockstep — do not leave stale "4".
- ⚠️ **Criterion 3 (Guard covers only what it knows)** applies to T1.5/T1.6 test additions — tests must exercise real product code paths, not mock-rebuilt implementations that pass trivially.
- ⚠️ **CLAUDE.md "单分支开发" vs AGENTS.md "分支隔离"** — currently working in `panel-tui-polish` worktree (manual creation). Per CLAUDE.md letter: prefer main + EnterWorktree; per AGENTS.md: explicit worktree isolation. Decision deferred to user at merge time. Doc-only changes (Plan 1) are reversible; code changes (this plan) warrant explicit user decision before merge.
- 📍 Subsystem routing references:
  - `interfaces/webchat/` → [DESKTOP_SHELL.md](docs/reference/DESKTOP_SHELL.md) · FL §4.7 §6.8 §6.9
  - `interfaces/tui/` `interfaces/cli/` `shared/protocol/` → FL §5.4 §5.11 §5.13 §5.23
  - `src/clarification/` `src/builtin_tools/ask_user.rs` → FL §5.3
  - `src/gateway/handlers/chat.rs` (T2.6) → [GATEWAY.md](docs/reference/GATEWAY.md) · FL §6.9
- 🔧 **Validation set** (per CLAUDE.md "最小可信验证集"): Each commit must include at minimum `cargo test -p <modified-crate> --lib`. Before final merge: full `cargo clippy --workspace --all-targets` + `cargo test -p alephcore --bins --no-run` to verify harness boot tests still compile.

---

## Review Focus

These inputs/failure modes are most likely to bite a person using the fixed code; each line gets a test pinned to its owning task:

1. **Race on reattach retry (T2.6)** — If Panel `reattach_after_connect` retries without proper guard, two `run_concurrency` calls may race. Pinned in Task B1.
2. **voice state machine ghost finish (T1.4)** — If pointer_up fires after Stop button mousedown, two `finish` calls double-emit. Pinned in Task A2.
3. **TUI subscribe_runtime_agents permanent freeze (T1.8)** — Single failure on reconnect never recovers. Pinned in Task A3.
4. **Model pin asymmetry (T1.9)** — TUI panel shows "Default" when user explicitly pinned. Pinned in Task A4.
5. **IME composition Enter误触发 (T1.2)** — Chinese IME candidate window Enter triggers send. Pinned in Task A6.
6. **A4 attachment chip wiring (T1.1+T1.10)** — Composer PendingAttachment not preserved in user-bubble history. Pinned in Task A9.
7. **Test infrastructure VERSION path (T1.11)** — TUI header test broken when run from worktree root. Pinned in Task A5.

---

## Phase 2 Tasks — Tier-1 Fixes (User-Visible)

### Task A1: T1.5 — Panel `approval_card.rs` 组件测试（薄覆盖区先填）

**Files:**
- Test: `interfaces/webchat/src/components/approval_card.rs` (new `#[cfg(test)] mod tests` inline)

**Interfaces:**
- Consumes: existing `approval_card::ApprovalCard` Leptos component, `ExecApprovalApi` trait (mockable), `i18n::t()`
- Produces: snapshot tests covering default render / pending state / expired state / approve button click / deny button click + reason

- [ ] **Step A1.1: Write the failing snapshot tests for default render**

In `interfaces/webchat/src/components/approval_card.rs`, add `#[cfg(test)] mod tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::render_component;
    use aleph_protocol::exec_approval::{ApprovalRequest, ApprovalDecision};
    use std::time::Duration;

    fn mock_request() -> ApprovalRequest {
        ApprovalRequest {
            id: "ap-1".into(),
            tool_name: "shell".into(),
            summary: "rm -rf build/".into(),
            preview: "rm -rf build/\nls -la".into(),
            tier: 1,
            timeout: Duration::from_secs(60),
        }
    }

    #[test]
    fn approval_card_renders_tool_name_and_summary() {
        let html = render_component(|| view! { <ApprovalCard request=mock_request() /> });
        assert!(html.contains("shell"));
        assert!(html.contains("rm -rf build/"));
    }

    #[test]
    fn approval_card_shows_remaining_seconds() {
        let html = render_component(|| view! { <ApprovalCard request=mock_request() /> });
        assert!(html.contains("60") || html.contains("剩余"));
    }

    #[test]
    fn approval_card_expired_state_renders_only_deny_button() {
        let mut r = mock_request();
        r.timeout = Duration::from_secs(0);
        // need tokio sleep first; see Step A1.2
    }
}
```

- [ ] **Step A1.2: Add `test_utils::render_component` helper**

Check `interfaces/webchat/src/test_utils.rs` for existing render helper. If absent, add minimal one:
```rust
pub fn render_component<F: FnOnce() -> V>(f: F) -> String where V: RenderHtml {
    // minimal: serialize the view to HTML and return
}
```

If helper doesn't exist and full implementation is heavy, **simplify**: render only the `view!` macro output to string by using `leptos::prelude::RenderHtml` (already in scope).

- [ ] **Step A1.3: Run tests, see RED**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-panel --lib components::approval_card 2>&1 | tail -30
```

Expected: 3 tests FAIL with "render_component not defined" or similar.

- [ ] **Step A1.4: Add the test_utils helper**

Read `interfaces/webchat/src/test_utils.rs` (if exists) or create it. Implement `render_component` using existing Leptos test scaffolding in this repo (look at existing snapshot tests in `messages.rs` for pattern).

- [ ] **Step A1.5: Run tests, see GREEN**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-panel --lib components::approval_card 2>&1 | tail -30
```

Expected: 3 tests PASS. If helper signature wrong, adjust minimally.

- [ ] **Step A1.6: Add 3 more tests** (expired state, approve click, deny-with-reason)

```rust
    #[tokio::test]
    async fn approval_card_expired_state_renders_only_deny_button() {
        let r = ApprovalRequest {
            id: "ap-1".into(),
            tool_name: "shell".into(),
            summary: "rm -rf".into(),
            preview: "rm".into(),
            tier: 1,
            timeout: Duration::from_secs(0),
        };
        let api = MockExecApprovalApi::new();
        let html = render_component(|| view! { <ApprovalCard request=r api=api /> });
        assert!(!html.contains("approve")); // approve button hidden
        assert!(html.contains("deny"));
    }

    #[tokio::test]
    async fn approval_card_approve_click_calls_api() {
        let api = MockExecApprovalApi::new();
        let _html = render_component(|| view! { <ApprovalCard request=mock_request() api=api.clone() /> });
        // trigger click
        api.expect_decide(ApprovalDecision::Approve { id: "ap-1".into() }).await;
    }

    #[tokio::test]
    async fn approval_card_deny_with_reason_persists_text() {
        let api = MockExecApprovalApi::new();
        let _html = render_component(|| view! { <ApprovalCard request=mock_request() api=api.clone() /> });
        api.expect_decide(ApprovalDecision::Deny { id: "ap-1".into(), reason: "too risky".into() }).await;
    }
```

Adapt exact signatures to the actual `ApprovalRequest` / `ApprovalDecision` / `ExecApprovalApi` types found in the codebase.

- [ ] **Step A1.7: Run tests, see GREEN for all 6**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-panel --lib components::approval_card 2>&1 | tail -20
```

Expected: 6 tests PASS.

- [ ] **Step A1.8: Commit**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add interfaces/webchat/src/components/approval_card.rs interfaces/webchat/src/test_utils.rs
git commit -m "panel: add ApprovalCard component tests (T1.5, was 0 tests)"
```

---

### Task A2: T1.4 — voice 录音状态机（先补测试后修）

**Files:**
- Test: `interfaces/webchat/src/platform/wide/views/chat/composer/voice.rs` (inline `#[cfg(test)] mod tests`)
- Modify: same file (fix ghost finish + timer cleanup)

**Interfaces:**
- Consumes: `RecState::{Idle,Starting,Recording,Transcribing}` enum; voice button event handlers
- Produces: state transition tests; bug fixes for `pointer_up` cleanup + `finish` Starting guard

- [ ] **Step A2.1: Write failing state-machine transition tests**

In `voice.rs` add `#[cfg(test)] mod tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_can_transition_to_starting_on_pointer_down() {
        let mut s = VoiceButtonState::new();
        assert_eq!(s.rec_state(), RecState::Idle);
        s.on_pointer_down();
        assert_eq!(s.rec_state(), RecState::Starting);
    }

    #[test]
    fn starting_can_transition_to_recording_when_backend_ready() {
        let mut s = VoiceButtonState::new();
        s.on_pointer_down(); // → Starting
        s.on_backend_ready(); // → Recording
        assert_eq!(s.rec_state(), RecState::Recording);
    }

    #[test]
    fn recording_can_transition_to_transcribing_on_pointer_up() {
        let mut s = VoiceButtonState::new();
        s.on_pointer_down();
        s.on_backend_ready();
        s.on_pointer_up(); // → Transcribing
        assert_eq!(s.rec_state(), RecState::Transcribing);
    }

    #[test]
    fn starting_pointer_up_does_not_emit_finish() {
        let mut s = VoiceButtonState::new();
        s.on_pointer_down(); // → Starting
        // backend never reports ready
        s.on_pointer_up(); // should NOT trigger finish (ghost)
        // explicit assertion: no recorder.stop() called
        assert!(s.finish_calls().is_empty(), "ghost finish on Starting state");
    }

    #[test]
    fn release_during_recording_clears_press_timer() {
        let mut s = VoiceButtonState::new();
        s.on_pointer_down();
        s.on_backend_ready();
        let timer_id = s.press_timer().id();
        s.on_pointer_up();
        assert!(s.press_timer().is_none(), "press_timer leaked after pointer_up");
        assert_ne!(timer_id, 0);
    }

    #[test]
    fn double_release_does_not_emit_double_finish() {
        let mut s = VoiceButtonState::new();
        s.on_pointer_down();
        s.on_backend_ready();
        s.on_pointer_up();
        let n = s.finish_calls().len();
        s.on_pointer_up(); // second release, should be ignored
        assert_eq!(s.finish_calls().len(), n, "double finish emitted");
    }
}
```

Adapt to the actual `VoiceButtonState` / `VoiceButtonApi` / etc. types. The key is each test pins ONE invariant.

- [ ] **Step A2.2: Run tests, see RED**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-panel --lib views::chat::composer::voice 2>&1 | tail -30
```

Expected: 6 tests FAIL.

- [ ] **Step A2.3: Implement minimal state machine scaffolding**

If `VoiceButtonState::finish_calls()` / `press_timer().id()` doesn't exist yet, add minimal accessors needed for the tests to compile + run.

- [ ] **Step A2.4: Run tests, see GREEN for the "correct path" tests**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-panel --lib views::chat::composer::voice 2>&1 | tail -20
```

Expected: 4 tests PASS (idle→starting→recording→transcribing, transitions clean).
Expected: 2 tests FAIL (starting_pointer_up_does_not_emit_finish, double_release_does_not_emit_double_finish) — these are the bugs we're about to fix.

- [ ] **Step A2.5: Fix bug 1 — pointer_up in Starting state**

In `voice.rs::on_pointer_up`:
```rust
fn on_pointer_up(&mut self) {
    if self.rec_state == RecState::Starting {
        return; // guard: don't emit finish when backend never ready
    }
    // ... existing logic
}
```

- [ ] **Step A2.6: Fix bug 2 — press_timer cleanup**

In `voice.rs::on_pointer_up`:
```rust
fn on_pointer_up(&mut self) {
    if let Some(timer) = self.press_timer.take() {
        self.window().clear_timeout(timer);
    }
    // ... existing logic
}
```

- [ ] **Step A2.7: Fix bug 3 — guard double-finish**

Track `finish_emitted: bool` flag; set true after first `recorder.stop()`; guard against re-entry.

- [ ] **Step A2.8: Run tests, see GREEN for all 6**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-panel --lib views::chat::composer::voice 2>&1 | tail -20
```

Expected: 6 tests PASS.

- [ ] **Step A2.9: Commit**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add interfaces/webchat/src/platform/wide/views/chat/composer/voice.rs
git commit -m "panel: voice button state machine — add 6 tests + fix ghost finish + timer leak (T1.4)"
```

---

### Task A3: T1.8 — TUI `subscribe_runtime_agents` 重连失败重试

**Files:**
- Test: `interfaces/tui/src/tui/app/tests.rs` (new test module)
- Modify: `interfaces/tui/src/tui/mod.rs` (subscribe failure branch)

**Interfaces:**
- Consumes: existing `state.runtime_agents_refetch_due` flag (already used for some retries); `subscribe_runtime_agents(state, client)` async fn
- Produces: failure branch sets `runtime_agents_refetch_due=true`; main loop retries on next tick

- [ ] **Step A3.1: Write failing test**

In `interfaces/tui/src/tui/app/tests.rs` add:

```rust
#[tokio::test]
async fn subscribe_runtime_agents_failure_sets_refetch_due_flag() {
    let mut state = AppState::new_for_test();
    state.runtime_agents_refetch_due = false;
    let client = MockClient::failing_subscribe();
    subscribe_runtime_agents(&mut state, &client).await;
    assert!(state.runtime_agents_refetch_due, "failure should set refetch_due");
}

#[tokio::test]
async fn main_loop_retries_subscribe_after_failure() {
    // Simulate: state.runtime_agents_refetch_due = true
    // Run one main_loop tick
    // Assert: subscribe_runtime_agents was called again
    // Assert: on success, runtime_agents_refetch_due = false
}
```

- [ ] **Step A3.2: Run tests, see RED**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-tui --lib app::tests::subscribe_runtime_agents 2>&1 | tail -30
```

Expected: 2 tests FAIL.

- [ ] **Step A3.3: Fix `subscribe_runtime_agents` failure path** in `tui/mod.rs:531-534`

```rust
match subscribe_runtime_agents(state, client).await {
    Ok(_) => { state.runtime_agents_refetch_due = false; }
    Err(e) => {
        tracing::warn!(error = ?e, "subscribe_runtime_agents failed; will retry");
        state.runtime_agents_refetch_due = true;
    }
}
```

- [ ] **Step A3.4: Fix main_loop to retry** in `tui/mod.rs::main_loop`

Check that main_loop checks `runtime_agents_refetch_due` at the top of each tick and calls `subscribe_runtime_agents` again if true.

- [ ] **Step A3.5: Run tests, see GREEN**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-tui --lib app::tests::subscribe_runtime_agents 2>&1 | tail -20
```

Expected: 2 tests PASS.

- [ ] **Step A3.6: Commit**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add interfaces/tui/src/tui/mod.rs interfaces/tui/src/tui/app/tests.rs
git commit -m "tui: retry subscribe_runtime_agents on reconnect failure (T1.8)"
```

---

### Task A4: T1.9 — TUI `SessionKnobs` 显示 `model_pin`

**Files:**
- Test: `interfaces/tui/src/tui/app/tests.rs` (new test for SessionKnobs 5-field shape)
- Modify: `interfaces/tui/src/tui/app/mod.rs:648` (add 5th field)
- Modify: status bar / `widgets/status_bar.rs` (render the new pin field)

**Interfaces:**
- Consumes: `SessionSnapshot.model_pin` (already in wire); `app::SessionKnobs<'a>` local 4-field struct
- Produces: 5-field SessionKnobs matching Panel; status bar renders pin model

- [ ] **Step A4.1: Write failing test**

```rust
#[test]
fn session_knobs_includes_model_pin_from_session_snapshot() {
    let snapshot = SessionSnapshot {
        exec_tier: Some("auto".into()),
        mode: "chat".into(),
        think_level: Some("medium".into()),
        memory_mode: Some("on".into()),
        model_pin: Some(("anthropic", "claude-opus-4-1-20250805").into()),
        // ... other fields
    };
    let knobs = AppState::session_knobs_from(&snapshot);
    assert_eq!(knobs.model_pin(), Some("claude-opus-4-1-20250805"));
}

#[test]
fn session_knobs_with_no_pin_returns_none() {
    let snapshot = SessionSnapshot {
        model_pin: None,
        // ...
    };
    assert!(AppState::session_knobs_from(&snapshot).model_pin().is_none());
}
```

- [ ] **Step A4.2: Run tests, see RED**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-tui --lib app::tests::session_knobs_includes_model_pin 2>&1 | tail -20
```

Expected: FAIL — `model_pin` field doesn't exist.

- [ ] **Step A4.3: Add `model_pin` to `SessionKnobs<'a>` struct in `app/mod.rs:648`**

```rust
pub struct SessionKnobs<'a> {
    pub mode: &'a str,
    pub exec_tier: Option<&'a str>,
    pub think_level: Option<&'a str>,
    pub memory_mode: Option<&'a str>,
    pub model_pin: Option<&'a str>,  // NEW
}
```

- [ ] **Step A4.4: Update `session_knobs()` to populate `model_pin`**

```rust
pub fn session_knobs(&self) -> SessionKnobs<'_> {
    let pin = self.session_snapshot.as_ref()
        .and_then(|s| s.model_pin.as_ref())
        .map(|p| p.1.as_str());  // (provider, model_id)
    SessionKnobs {
        mode: ...,
        exec_tier: ...,
        think_level: ...,
        memory_mode: ...,
        model_pin: pin,
    }
}
```

- [ ] **Step A4.5: Update `widgets/status_bar.rs` to render the pin**

Find the render code that iterates `SessionKnob::ALL` (4 variants) and add a 5th segment when `model_pin.is_some()`:
```rust
if let Some(pin) = knobs.model_pin {
    segments.push(format!("📌 {}", pin));
}
```

Or follow existing color/style pattern. Keep change <40 lines total.

- [ ] **Step A4.6: Add `slash::SessionKnob::ModelPin` variant** if `slash::ALL` is exhaustively used

If the code does `match knob { SessionKnob::ExecTier => ..., SessionKnob::Mode => ..., ... }` exhaustively, add the new variant.

- [ ] **Step A4.7: Run tests, see GREEN**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-tui --lib app::tests::session_knobs 2>&1 | tail -20
```

Expected: 2 tests PASS.

- [ ] **Step A4.8: Commit**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add interfaces/tui/src/tui/app/mod.rs interfaces/tui/src/tui/widgets/status_bar.rs interfaces/tui/src/tui/slash.rs interfaces/tui/src/tui/app/tests.rs
git commit -m "tui: SessionKnobs now includes model_pin (T1.9, was 4 fields, Panel has 5)"
```

---

### Task A5: T1.11 — TUI header VERSION path 用 `CARGO_MANIFEST_DIR`

**Files:**
- Test: `interfaces/tui/src/tui/widgets/header.rs` (existing test, fix path)

**Interfaces:**
- Consumes: `../../VERSION` (current relative path)
- Produces: `env!("CARGO_MANIFEST_DIR") + "/../../VERSION"` (absolute, robust)

- [ ] **Step A5.1: Write failing test** (already exists in `header.rs:201`)

The existing test is the failing one. Verify it FAILS from worktree root:
```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-tui --lib widgets::header::tests::the_version_comes_from_the_version_file_not_the_cargo_copy 2>&1 | tail -15
```

Expected: FAIL with "Os { code: 2, kind: NotFound }".

- [ ] **Step A5.2: Fix path in `header.rs:201`**

```rust
#[test]
fn the_version_comes_from_the_version_file_not_the_cargo_copy() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let from_file = std::fs::read_to_string(format!("{}/../../VERSION", manifest_dir))
        .expect("workspace VERSION file");
    assert_eq!(VERSION, from_file.trim());
}
```

- [ ] **Step A5.3: Run test, see GREEN**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-tui --lib widgets::header::tests::the_version_comes_from_the_version_file_not_the_cargo_copy 2>&1 | tail -15
```

Expected: PASS.

- [ ] **Step A5.4: Commit**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add interfaces/tui/src/tui/widgets/header.rs
git commit -m "tui: header VERSION test uses CARGO_MANIFEST_DIR (T1.11, was relative path broken)"
```

---

### Task A6: T1.2 — Panel composer IME composition 闸

**Files:**
- Test: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs` (inline test)
- Modify: same file (compositionstart/end listener + 100ms闸)

**Interfaces:**
- Consumes: existing composer `on:keydown` handler at `composer/mod.rs:1078-1118`
- Produces: 100ms "recently settled" window after `compositionend` that suppresses Enter→send

- [ ] **Step A6.1: Write failing test**

```rust
#[cfg(test)]
mod ime_tests {
    use super::*;

    #[test]
    fn enter_during_active_composition_does_not_send() {
        // Simulate: compositionstart → keydown(Enter) → no send
        let mut s = ComposerState::new_for_test();
        s.on_composition_start();
        s.on_keydown("Enter", /* shift = */ false, /* is_composing = */ false);
        assert!(s.send_calls().is_empty());
    }

    #[test]
    fn enter_within_100ms_of_composition_end_does_not_send() {
        let mut s = ComposerState::new_for_test();
        s.on_composition_start();
        s.on_composition_end();
        // immediately keydown Enter — within 100ms window
        s.on_keydown("Enter", false, false);
        assert!(s.send_calls().is_empty());
    }

    #[test]
    fn enter_after_100ms_of_composition_end_sends() {
        let mut s = ComposerState::new_for_test();
        s.on_composition_start();
        s.on_composition_end();
        s.advance_time_ms(150);
        s.on_keydown("Enter", false, false);
        assert_eq!(s.send_calls().len(), 1);
    }
}
```

- [ ] **Step A6.2: Run tests, see RED**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-panel --lib views::chat::composer::ime_tests 2>&1 | tail -20
```

Expected: 3 tests FAIL.

- [ ] **Step A6.3: Add IME state tracking to ComposerState**

Add fields: `composition_active: bool`, `last_composition_end_ms: Option<u64>` (use a clock provider for testability).

- [ ] **Step A6.4: Add compositionstart/end listeners**

```rust
on:compositionstart=move |_| { state.composition_active = true; }
on:compositionend=move |_| {
    state.composition_active = false;
    state.last_composition_end_ms = Some(clock.now_ms());
}
```

- [ ] **Step A6.5: Gate Enter handling**

```rust
let recently_settled = state.last_composition_end_ms
    .map(|t| clock.now_ms().saturating_sub(t) < 100)
    .unwrap_or(false);
if ev.key() == "Enter" && !ev.shift_key() && !state.composition_active && !recently_settled {
    ev.prevent_default();
    // send
}
```

- [ ] **Step A6.6: Run tests, see GREEN**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-panel --lib views::chat::composer::ime_tests 2>&1 | tail -20
```

Expected: 3 tests PASS.

- [ ] **Step A6.7: Commit**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs
git commit -m "panel: IME composition gate prevents Enter during Chinese candidate window (T1.2)"
```

---

### Task A7: T1.3 — Panel attachments `<For>` key 漂移

**Files:**
- Test: `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs`
- Modify: same file

- [ ] **Step A7.1: Write failing test**

```rust
#[test]
fn attachment_for_key_uses_stable_hash_not_index() {
    let attachments = vec![
        Attachment { name: "a.txt".into(), size: 100, mime_type: "text/plain".into(), data_base64: "...".into() },
        Attachment { name: "b.txt".into(), size: 200, mime_type: "text/plain".into(), data_base64: "...".into() },
        Attachment { name: "c.txt".into(), size: 300, mime_type: "text/plain".into(), data_base64: "...".into() },
    ];
    let key1 = attachment_key(&attachments, 1);
    // remove attachment at idx 1; b.txt is gone
    let mut reduced = attachments.clone();
    reduced.remove(1);
    // The remaining "c.txt" was at idx 2, now at idx 1; its key should NOT equal key1 (which was for "b.txt")
    let key_after_remove = attachment_key(&reduced, 1);
    assert_ne!(key1, key_after_remove);
}

#[test]
fn attachment_key_is_deterministic() {
    let att = Attachment { name: "x.png".into(), size: 999, .. };
    assert_eq!(attachment_key(&[att.clone()], 0), attachment_key(&[att], 0));
}
```

- [ ] **Step A7.2: Run tests, see RED**

- [ ] **Step A7.3: Change `<For key=|(idx, f)| format!("{}:{}", idx, f.name)>` to use stable hash**

```rust
let key = |(att, _idx): &(Attachment, usize)| -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    att.name.hash(&mut h);
    att.size.hash(&mut h);
    format!("{:x}", h.finish())
};
```

- [ ] **Step A7.4: Run tests, see GREEN**

- [ ] **Step A7.5: Commit**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs
git commit -m "panel: attachments For key uses stable hash not index (T1.3)"
```

---

### Task A8: T1.7 — 团队聊天附件拒绝后恢复

**Files:**
- Test: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs` (inline test)
- Modify: same file (team branch send failure path)

- [ ] **Step A8.1: Write failing test**

```rust
#[test]
fn team_chat_attachment_rejection_restores_tray() {
    let mut s = ComposerState::new_for_test();
    s.set_team_id(Some("team-x".into()));
    s.attachments.set(vec![Attachment { name: "report.pdf".into(), size: 500, ..default() }]);
    s.text.set("here is the doc".into());
    // simulate send failure in team branch
    let api = MockApi::failing_send();
    s.send_message(&api);
    // attachment preserved, not cleared
    assert_eq!(s.attachments.get().len(), 1);
    assert_eq!(s.attachments.get()[0].name, "report.pdf");
    assert!(s.send_error.get().is_some(), "send error should be displayed");
}
```

- [ ] **Step A8.2: Run test, see RED**

- [ ] **Step A8.3: Fix team branch send failure path** in `composer/mod.rs::send_message`

Find the team-chat branch:
```rust
if let Some(team_id) = ... {
    if !attachments.is_empty() {
        chat.set_send_error("team chat does not accept attachments");
        // FIX: don't clear attachments
        // attachments.set(Vec::new());  // ← remove this line
        return;
    }
}
```

Or follow whichever pattern the existing `failed_send_restores_the_tray` test uses for single-chat; mirror it.

- [ ] **Step A8.4: Run test, see GREEN**

- [ ] **Step A8.5: Commit**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs
git commit -m "panel: team chat attachment rejection preserves tray (T1.7)"
```

---

### Task A9: T1.1 + T1.10 — A4 附件 chip（Panel + TUI）

**Files:**
- Test: Panel `interfaces/webchat/src/platform/wide/views/chat/messages.rs` (snapshot: chip in user bubble history)
- Test: TUI `interfaces/tui/src/tui/app/tests.rs` (snapshot: chip rendering)
- Modify: Panel user-bubble render (show sent attachments as chips)
- Modify: TUI `TranscriptEntry::UserText` add `attachments` field; `add_user_message` accepts attachment list

**Interfaces:**
- Consumes: existing `PendingAttachment` (Panel composer); `m.content` already serialized
- Produces: 
  - Panel: user bubble shows attachment chips below text
  - TUI: `TranscriptEntry::UserText { text, at_ms, attachments: Vec<TuiAttachment> }` (TuiAttachment = { name, mime })

- [ ] **Step A9.1: Write failing Panel snapshot test**

```rust
#[test]
fn user_bubble_renders_sent_attachments_as_chips() {
    let mut msg = UserMessage::for_test();
    msg.text = "see attached".into();
    msg.attachments = vec![Attachment::png("photo.png", 1024)];
    let html = render_component(|| view! { <MessageBubble message=msg /> });
    assert!(html.contains("photo.png"));
    assert!(html.contains("see attached"));
}
```

- [ ] **Step A9.2: Run test, see RED**

- [ ] **Step A9.3: Add attachment field to user-side MessageView projection**

Find the user-message projection in `events.rs` (or wherever `MessageBubble`'s prop type is constructed). Add `attachments: Vec<AttachmentMeta>` (just name + mime, not full data).

- [ ] **Step A9.4: Update MessageBubble user臂 to render chips**

In `messages.rs:1246-1254`, after the `whitespace-pre-wrap` text div, render attachment chips:
```rust
{message.attachments.iter().map(|a| view! {
    <div class="attachment-chip">
        <span class="name">{&a.name}</span>
        <span class="mime">{&a.mime}</span>
    </div>
}).collect::<Vec<_>>()}
```

- [ ] **Step A9.5: Run Panel test, see GREEN**

- [ ] **Step A9.6: Write failing TUI test**

```rust
#[test]
fn user_text_transcript_entry_can_carry_attachments() {
    let entry = TranscriptEntry::UserText {
        id: EntryId::next(),
        text: "see attached".into(),
        at_ms: 0,
        attachments: vec![TuiAttachment { name: "photo.png".into(), mime: "image/png".into() }],
    };
    let rendered = render_transcript_entry(&entry);
    assert!(rendered.contains("photo.png"));
}
```

- [ ] **Step A9.7: Add `TuiAttachment` struct + extend `TranscriptEntry::UserText`**

```rust
#[derive(Clone, Debug)]
pub struct TuiAttachment {
    pub name: String,
    pub mime: String,
}

// in enum TranscriptEntry:
UserText {
    id: EntryId,
    text: String,
    at_ms: u64,
    attachments: Vec<TuiAttachment>,  // NEW
},
```

- [ ] **Step A9.8: Update `add_user_message` to accept attachments**

```rust
pub fn add_user_message(&mut self, content: String, attachments: Vec<TuiAttachment>) {
    let id = self.next_entry_id();
    self.messages.push(TranscriptEntry::UserText {
        id, text: content, at_ms: as_ms(Utc::now()), attachments,
    });
    self.sends = self.sends.saturating_add(1);
}
```

Update all callers (use grep to find).

- [ ] **Step A9.9: Update user-text rendering in `chat_area.rs` to show attachment chips**

After the text content, render chip lines:
```rust
for att in &entry.attachments {
    lines.push(format!("  📎 {} ({})", att.name, att.mime));
}
```

- [ ] **Step A9.10: Run TUI test, see GREEN**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=1 cargo test -p aleph-tui --lib app::tests::user_text_transcript_entry 2>&1 | tail -20
```

- [ ] **Step A9.11: Commit**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add interfaces/webchat/src/platform/wide/views/chat/messages.rs interfaces/webchat/src/platform/wide/views/chat/events.rs interfaces/tui/src/tui/app/mod.rs interfaces/tui/src/tui/widgets/chat_area.rs interfaces/tui/src/tui/app/tests.rs
git commit -m "panel+tui: user bubble shows sent attachments as chips (T1.1+T1.10)"
```

---

## Phase 3 Tasks — Tier-2 加固 + Playwright e2e

### Task B1: T2.6 — Panel `reattach_after_connect` 失败重试

**Files:**
- Test: `interfaces/webchat/src/state/reattach.rs` (new test)
- Modify: same file (add `pending_reattach` flag + retry logic)

- [ ] **Step B1.1: Write failing test**

```rust
#[tokio::test]
async fn reattach_after_connect_records_failure_and_sets_pending_flag() {
    let api = MockApi::failing_run_concurrency();
    reattach_after_connect(&api, &state).await;
    assert!(state.pending_reattach.get(), "pending_reattach should be set");
}

#[tokio::test]
async fn next_connection_epoch_triggers_reattach_when_pending() {
    let mut state = TestState::new();
    state.pending_reattach.set(true);
    let api = MockApi::ok_run_concurrency();
    state.bump_connection_epoch();
    // assert run_concurrency was invoked and pending_reattach cleared
}
```

- [ ] **Step B1.2: Run tests, see RED**

- [ ] **Step B1.3: Add `pending_reattach: RwSignal<bool>` to ChatState**

- [ ] **Step B1.4: Update `reattach_after_connect` to set the flag on failure**

```rust
match run_concurrency(&api).await {
    Ok(...) => { state.pending_reattach.set(false); ... }
    Err(e) => {
        tracing::warn!(?e, "reattach failed");
        state.pending_reattach.set(true);
        return;
    }
}
```

- [ ] **Step B1.5: Update app root Effect to check `pending_reattach` after epoch bump**

```rust
create_effect(move |_| {
    let _ = connection_epoch.get();
    if state.pending_reattach.get() {
        spawn(reattach_after_connect(...));
        state.pending_reattach.set(false);
    }
});
```

- [ ] **Step B1.6: Run tests, see GREEN**

- [ ] **Step B1.7: Commit**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add interfaces/webchat/src/state/reattach.rs interfaces/webchat/src/state/mod.rs
git commit -m "panel: reattach retries on next connection_epoch when run_concurrency fails (T2.6)"
```

---

### Task B2: T2.4 — TUI `command_palette` ratatui snapshot 测试

**Files:**
- Test: `interfaces/tui/src/tui/widgets/command_palette.rs` (TestBackend snapshot)

- [ ] **Step B2.1: Write failing snapshot tests**

```rust
#[test]
fn command_palette_renders_empty_state() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| {
        render_command_palette(f, f.size(), &PaletteState::empty());
    }).unwrap();
    let buffer = terminal.backend().buffer().clone();
    insta::assert_snapshot!(buffer_to_string(&buffer));
}

#[test]
fn command_palette_highlights_selected_item() {
    // ... similar
}
```

- [ ] **Step B2.2: Run tests, see RED**

- [ ] **Step B2.3: Implement snapshot rendering helpers** (use existing `widgets::input_area` tests as template)

- [ ] **Step B2.4: Run tests, see GREEN; commit**

```bash
git commit -m "tui: command_palette snapshot tests (T2.4)"
```

---

### Task B3: T2.7-T2.11 — Track C 加固单测（5 个）

**Files:** `interfaces/webchat/src/state/sessions.rs` + `interfaces/tui/src/tui/app/tests.rs`

- [ ] **Step B3.1: T2.7 — `connection_epoch` 单调性单测**（Panel）

```rust
#[test]
fn connection_epoch_is_monotone_across_reconnect() {
    let mut state = TestState::new();
    let e0 = state.connection_epoch.get();
    state.bump_connection_epoch();
    let e1 = state.connection_epoch.get();
    state.bump_connection_epoch();
    let e2 = state.connection_epoch.get();
    state.bump_connection_epoch();
    let e3 = state.connection_epoch.get();
    assert!(e1 > e0 && e2 > e1 && e3 > e2);
}
```

- [ ] **Step B3.2: T2.8 — `bind_run` 重复调用不双计**（Panel）

```rust
#[test]
fn bind_run_called_twice_does_not_double_count_running() {
    let mut state = TestState::new();
    state.bind_run("run-1");
    state.bind_run("run-1");  // second call
    assert_eq!(state.running_count(), 1);
}
```

Requires adding `if route.contains(run_id) return;` guard to `bind_run`.

- [ ] **Step B3.3: T2.9 — TUI elapsed 计时器终值**（TUI）

If product confirms "elapsed should freeze at completion": change `run_started_at` to be set to a sentinel on RunComplete; add test that assert `run_started_at.is_some()` after completion.

If product confirms "elapsed should disappear": leave behavior, add test that pins current behavior.

Either way, write the test that pins the chosen behavior.

- [ ] **Step B3.4: T2.10 — `mark_queued` 乱序单测**（Panel）

```rust
#[test]
fn run_accepted_after_run_queued_does_not_flip_back_to_queued() {
    let mut state = TestState::new();
    state.handle_run_accepted("run-1");
    state.handle_run_queued("run-1");  // out of order
    assert_eq!(state.run_phase(), ChatPhase::Thinking);
}
```

- [ ] **Step B3.5: T2.11 — 跨会话切回端到端**（Panel）

```rust
#[test]
fn switching_back_to_a_session_recovers_its_run_route() {
    let mut state = TestState::new();
    state.bind_run("run-1");
    state.set_session_key("s1");
    state.set_session_key("s2");
    state.set_session_key("s1");  // switch back
    assert!(state.route_lookup("run-1").is_some());
}
```

- [ ] **Step B3.6: Run all 5 tests; commit**

```bash
git commit -m "panel+tui: C-track加固单测 (T2.7-T2.11)"
```

---

### Task B4: T2.12 — Panel `events.rs::parse_run_halt` 等 string-dispatch 静态覆盖断言

**Files:**
- Test: `interfaces/webchat/src/platform/wide/views/chat/events.rs` (new source-scanner test)

- [ ] **Step B4.1: Write failing source-scanner test**

```rust
#[test]
fn every_string_dispatch_branch_in_events_has_a_unit_test() {
    // Use syn to parse events.rs
    // Find all `event_type == "..."` literals
    // Assert each has a corresponding #[test] in the test module
    let file = std::fs::read_to_string(file!()).unwrap();
    let dispatch_strings: Vec<String> = extract_dispatch_strings(&file);
    for s in dispatch_strings {
        assert!(
            file.contains(&format!("test_{}", sanitize(&s))),
            "string dispatch '{}' has no unit test; need one named `test_{}`.", s, sanitize(&s)
        );
    }
}
```

- [ ] **Step B4.2: Run test, see RED** (some string dispatches don't have matching tests)

- [ ] **Step B4.3: Add missing tests** for any unmatched dispatch strings

- [ ] **Step B4.4: Run test, see GREEN**

- [ ] **Step B4.5: Commit**

```bash
git commit -m "panel: static guard that every event string-dispatch branch has a unit test (T2.12)"
```

---

### Task B5: T2.13 — Panel 消费 `StreamEvent::ReasoningBlock` + `UncertaintySignal`

> ⚠️ **GATE**: Before this task, verify with user whether Panel SHOULD consume these events. If product says no, document and skip. If yes, proceed.

- [ ] **Step B5.1: Confirm with user**

Ask user via ask_user: "Should Panel consume `StreamEvent::ReasoningBlock` and `UncertaintySignal`? (TUI does; Panel doesn't)"

If yes → continue; if no → write a documentation note in `events.rs` and commit.

- [ ] **Step B5.2: Write failing test** (only if yes)

```rust
#[test]
fn reasoning_block_event_updates_reasoning_panel() {
    let mut state = TestState::new();
    state.handle_stream_event(StreamEvent::ReasoningBlock {
        run_id: "run-1".into(),
        chunk_id: "ch-1".into(),
        text: "thinking...".into(),
    });
    assert!(state.reasoning_panel_visible("run-1"));
}
```

- [ ] **Step B5.3: Add the dispatch branch** in `events.rs`

```rust
"reasoning_block" => {
    state.update_reasoning(...);
}
```

- [ ] **Step B5.4: Run test, see GREEN**

- [ ] **Step B5.5: Commit**

---

### Task B6: T2.1 — Playwright e2e: streaming echo 中断恢复

> ⚠️ **GATE**: Requires `npm install` + Playwright browser binaries + running `aleph-server` on port 18791. Per Plan 1 ruling, infra not ready in Phase 0. If user wants Phase 3 e2e, must first run:
> ```bash
> cd /home/zou/data/workspace/Aleph-panel-tui-polish
> npm install --no-audit --no-fund  # may take 5+ minutes
> npx playwright install chromium  # downloads ~150MB browser
> ```
> Then build the `aleph-server` binary if not present.

- [ ] **Step B6.1: Verify infra ready**

```bash
ls /home/zou/data/workspace/Aleph-panel-tui-polish/node_modules/@playwright/test 2>/dev/null || echo "node_modules not ready"
```

If not ready: ask user to run `npm install` and `npx playwright install chromium` before this task.

- [ ] **Step B6.2: Write failing e2e spec**

Create `tests/e2e/tests/streaming-echo-resume.spec.ts`:
```typescript
import { test, expect } from '@playwright/test';

test('streaming echo resumes after reconnect', async ({ page }) => {
    await page.goto('/');
    await page.fill('[data-testid="composer"]', 'Tell me a long story');
    await page.click('[data-testid="send"]');
    
    // wait for streaming to start
    await expect(page.locator('[data-testid="streaming-indicator"]')).toBeVisible();
    await page.waitForTimeout(2000);
    
    // simulate connection drop
    await page.evaluate(() => window.dispatchEvent(new Event('offline')));
    await page.waitForTimeout(500);
    
    // simulate reconnect
    await page.evaluate(() => window.dispatchEvent(new Event('online')));
    await page.waitForTimeout(2000);
    
    // the streaming should continue or complete, not stuck
    const content = await page.locator('[data-testid="last-message"]').textContent();
    expect(content?.length || 0).toBeGreaterThan(50);
});
```

- [ ] **Step B6.3: Run spec, see RED**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
npx playwright test tests/e2e/tests/streaming-echo-resume.spec.ts 2>&1 | tail -30
```

- [ ] **Step B6.4: Fix any bugs surfaced** (likely minimal, since FEATURE_LOCATOR marks this ✅)

- [ ] **Step B6.5: Run spec, see GREEN**

- [ ] **Step B6.6: Commit**

```bash
git add tests/e2e/tests/streaming-echo-resume.spec.ts
git commit -m "e2e: streaming echo resume after reconnect (T2.1)"
```

---

### Task B7: T2.2 — TUI `halt_notice` 启动期 locale 兜底

**Files:**
- Test: `interfaces/tui/src/tui/app/tests.rs`
- Modify: `interfaces/tui/src/tui/app/events.rs::halt_notice`

- [ ] **Step B7.1: Write failing test**

```rust
#[test]
fn halt_notice_falls_back_when_locale_is_unset() {
    let token = "halt-token-x";
    let notice = halt_notice(token, None);
    assert!(!notice.is_empty());
    assert!(notice.contains("halt-token-x") || notice.contains(token));
}
```

- [ ] **Step B7.2: Run test, see RED**

- [ ] **Step B7.3: Add default locale in halt_notice**

```rust
pub fn halt_notice(token: &str, locale: Option<Locale>) -> String {
    let locale = locale.unwrap_or(Locale::En); // ← fallback
    terminate::label(token, locale).to_string()
}
```

- [ ] **Step B7.4: Run test, see GREEN**

- [ ] **Step B7.5: Commit**

---

### Task B8: T2.5 — TUI `dialog` 滚动指示 + `sent-history` 翻页边界

**Files:**
- Test: `interfaces/tui/src/tui/widgets/dialog.rs` + `interfaces/tui/src/tui/commands.rs`

- [ ] **Step B8.1: Write failing tests** (2-3 tests each)

- [ ] **Step B8.2: Run tests, see RED**

- [ ] **Step B8.3: Add scroll indicator in dialog when options > visible**

- [ ] **Step B8.4: Add sent-history pagination boundary tests**

- [ ] **Step B8.5: Run tests, see GREEN**

- [ ] **Step B8.6: Commit**

---

## Phase 4 Tasks — Docs Sync

### Task C1: 更新 FEATURE_LOCATOR.md（Tier-1 + Tier-2 修复状态）

**Files:**
- Modify: `docs/reference/FEATURE_LOCATOR.md` (in worktree)

- [ ] **Step C1.1: Update entries this round fixed**

For each Tier-1/Tier-2 commit, update FEATURE_LOCATOR.md status from ⚠️/❌ to ✅ (if appropriate) and append `(§X.YZ, 2026-09-21 panel-tui-polish)` marker.

Specific entries likely to flip:
- TUI header VERSION test → ✅ (§X.YZ, 2026-09-21)
- TUI subscribe retry → ✅
- TUI model_pin display → ✅

(Verify by reading FEATURE_LOCATOR.md for the relevant rows.)

- [ ] **Step C1.2: Run grep to verify no ❌/⚠️ entries remain stale**

```bash
grep -n "❌\|⚠️" docs/reference/FEATURE_LOCATOR.md | head -30
```

For each remaining ⚠️/❌, decide: (a) was out of L1 scope → leave alone; (b) was missed → add new entry in this round.

- [ ] **Step C1.3: Commit**

```bash
git add docs/reference/FEATURE_LOCATOR.md
git commit -m "docs: update FEATURE_LOCATOR entries for T1.x/T2.x fixes (2026-09-21)"
```

---

### Task C2: CHANGELOG entry

**Files:**
- Modify: `CHANGELOG.md` (in worktree)

- [ ] **Step C2.1: Add entry under current date**

```
## 2026-09-21 — Panel & TUI polish round 1 (L1, audit-first)

### Fixes
- panel: voice button state machine ghost finish + timer leak (T1.4)
- panel: IME composition gate prevents Enter during Chinese candidate window (T1.2)
- panel: attachments For key uses stable hash (T1.3)
- panel: team chat attachment rejection preserves tray (T1.7)
- panel: approval_card component tests added (T1.5)
- panel: ask.rs test density boost (T1.6)
- panel: reattach retries on next connection_epoch (T2.6)
- panel: event string-dispatch static coverage guard (T2.12)
- tui: subscribe_runtime_agents retries on reconnect failure (T1.8)
- tui: SessionKnobs now includes model_pin (T1.9)
- tui: header VERSION test uses CARGO_MANIFEST_DIR (T1.11)
- panel+tui: user bubble shows sent attachments as chips (T1.1, T1.10)

### Tests
- Added 30+ unit tests across 8 modules (thin coverage hardening)

### Out of scope (deferred to next round)
- True image rendering (wire protocol change)
- A3 reasoning跨会话持久化 (wire change)
- pi-ask-user UX mode additions (new features)
```

- [ ] **Step C2.2: Commit**

```bash
git add CHANGELOG.md
git commit -m "changelog: Panel & TUI polish round 1 entry (2026-09-21)"
```

---

## Final Task — Whole-branch review

**Per writing-plans/executing-plans skill:** After all tasks complete, dispatch a fresh reviewer on the most capable model (via `requesting-code-review` skill) for whole-branch review.

- [ ] **Step Final.1: Generate review package**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
bash /home/zou/.pi/agent/git/github.com/obra/superpowers/skills/subagent-driven-development/scripts/review-package docs/superpowers/plans/2026-09-21-panel-tui-polish-fixes.md $(git merge-base main HEAD) HEAD
```

- [ ] **Step Final.2: Dispatch reviewer subagent** with the package + plan + spec paths + Review Focus verbatim.

- [ ] **Step Final.3: Re-grade + apply Critical/Important fixes** (one pass each, RED→GREEN).

- [ ] **Step Final.4: Print final report** with Rulings list, Deferred minors list, success criteria.

- [ ] **Step Final.5: Wait for user to merge via `finishing-a-development-branch` skill**

---

## Plan summary

| Phase | Tasks | Estimated lines | Risk |
|-------|-------|-----------------|------|
| **Phase 2** (Tier-1 fixes) | A1–A9 | 250-450 lines (mostly tests) | Low (each fix has TDD test) |
| **Phase 3** (Tier-2 + e2e) | B1–B8 | 200-300 lines | Medium (B6 Playwright infra; B5 user gate) |
| **Phase 4** (docs) | C1, C2 | <50 lines | Trivial |
| **Final** (review) | Final | — | Standard |

**Total**: 24 items across 19 tasks (Phase 2: 9, Phase 3: 8, Phase 4: 2).

**Risk gates**:
- B5 (T2.13) requires user confirmation before proceeding
- B6 (Playwright e2e) requires `npm install` + browser binaries (~150MB) before running