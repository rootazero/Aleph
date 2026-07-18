# Workspace Panel Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the Panel's right-side workspace pane from a click-to-view tool-JSON inspector into an auto-populating Agent execution activity stream, with a project file-tree drawer for previewing files.

**Architecture:** Phase 1 rewrites the Panel-only state + view to derive an activity timeline reactively from existing `ChatState.messages` + `WorkspaceState.tool_payloads` (zero Core change). Phase 2 adds one Core RPC (`fs.read_file`) and a collapsible file-tree drawer that reuses the existing `fs.list_dir` browse surface. All data flows Core→Panel via JSON-RPC (redline R4); the Panel never computes business logic.

**Tech Stack:** Rust, Leptos (WASM, `leptos::prelude::*`), Tailwind classes, JSON-RPC gateway handlers, `serde_json`.

---

## Background

Current state (verified):
- `interfaces/webchat/src/state/layout.rs` defines `WorkspaceContent { Empty, ToolDetail { run_id, tool_id } }` + `WorkspaceState`.
- `interfaces/webchat/src/components/workspace_panel.rs` renders `Empty` (hero) or `ToolDetail` (tool renderer + `PayloadBlock` JSON viewer).
- The ONLY fill path is `views/chat/messages.rs:~307` → `WorkspaceState::show_tool` on a tool-chip click. So the pane is almost always the empty hero.
- `app.rs:285-292` renders a chrome-band label that matches on `WorkspaceContent`.
- `ChatState.messages: RwSignal<Vec<ChatMessage>>`; each `ChatMessage` has `id` (`"assistant-{run_id}"`) and `tool_calls: Vec<ToolCallEntry>`. `ToolCallEntry { tool_id, tool_name, status, duration_ms }`.
- `WorkspaceState.tool_payloads: HashMap<(run_id,tool_id), ToolPayload { args, result }>` already captured by `views/chat/events.rs`.
- Core `src/gateway/handlers/fs.rs` exposes `fs.allowed_roots / fs.home_dir / fs.list_dir / fs.create_dir`, all gated by `projects.allowed_roots`. There is NO `fs.read_file`.
- `chat.active_project_root: RwSignal<Option<String>>` holds the current project folder.

## File Structure

**Phase 1 (Panel only):**
- Modify: `interfaces/webchat/src/state/layout.rs` — drop `WorkspaceContent`, add timeline/drawer state + methods.
- Modify: `interfaces/webchat/src/components/workspace_panel.rs` — render `ActivityTimeline`; keep `PayloadBlock`/`find_tool_entry`.
- Modify: `interfaces/webchat/src/app.rs:285-292` — replace `WorkspaceContent` band label.
- Modify: `interfaces/webchat/src/views/chat/messages.rs` — rename `show_tool` call → `focus_tool_row`.
- Modify: `interfaces/webchat/src/views/chat/events.rs` — bump activity badge on tool start.
- Modify: `interfaces/webchat/src/components/layout_toggle.rs` — unseen-activity dot.

**Phase 2 (Core + Panel):**
- Modify: `src/gateway/handlers/fs.rs` — add `handle_read_file`.
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/settings.rs` — register `fs.read_file`.
- Modify: `interfaces/webchat/src/api/fs.rs` — add `FsApi::read_file` + `FilePreview`/`ReadFileResult`.
- Modify: `interfaces/webchat/src/components/workspace_panel.rs` — add `FilesDrawer` sub-view.
- Modify: `interfaces/webchat/src/state/layout.rs` — `FilePreview`, drawer/select methods.

## Commands

- Panel compile check: `cargo check -p aleph-panel` (the webchat crate). If the crate name differs, use `just wasm` to compile WASM.
- Panel unit tests: `cargo test -p aleph-panel --lib`
- Core compile check: `cargo check -p alephcore`
- Core tests: `cargo test -p alephcore --lib gateway::handlers::fs`

> If `aleph-panel` is not the crate name, run `grep '^name' interfaces/webchat/Cargo.toml` to get it and substitute throughout.

---

## Phase 1 — Auto Activity Stream (no Core change)

### Task 1: Replace `WorkspaceContent` with timeline/drawer state

**Files:**
- Modify: `interfaces/webchat/src/state/layout.rs`

- [ ] **Step 1: Update imports and add `FilePreview` + remove `WorkspaceContent`**

In `interfaces/webchat/src/state/layout.rs`, change the imports line to include `HashSet`:

```rust
use std::collections::{HashMap, HashSet};
```

Delete the entire `WorkspaceContent` enum (the `#[derive(...)] #[derive(Default)] pub enum WorkspaceContent { Empty, ToolDetail {...} }` block).

Add this new type in its place:

```rust
/// A lazily-loaded file preview shown in the files drawer (Phase 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePreview {
    pub path: String,
    pub content: String,
    pub truncated: bool,
}
```

- [ ] **Step 2: Rewrite the `WorkspaceState` struct fields**

Replace the `WorkspaceState` struct definition with:

```rust
/// Reactive workspace state. Provided once via context, cloned via `Copy`.
#[derive(Clone, Copy)]
pub struct WorkspaceState {
    pub mode: RwSignal<LayoutMode>,
    /// Captured tool-call args + results keyed by `(run_id, tool_id)`.
    /// Populated by `events::subscribe_run_events`.
    pub tool_payloads: RwSignal<HashMap<(String, String), ToolPayload>>,
    /// `tool_id`s whose activity-timeline row is expanded inline.
    pub expanded_events: RwSignal<HashSet<String>>,
    /// Count of tool activities that started while the pane was not in
    /// Split — drives the toggle button's unseen-activity dot (R5: we
    /// surface activity without force-opening the pane).
    pub unseen_activity: RwSignal<usize>,
    /// A `tool_id` the user clicked from a chat chip — the timeline scrolls
    /// to / expands it. Cleared once consumed.
    pub focus_tool: RwSignal<Option<String>>,
    /// Whether the bottom files drawer is expanded (Phase 2).
    pub files_drawer_open: RwSignal<bool>,
    /// Currently previewed file (Phase 2).
    pub selected_file: RwSignal<Option<FilePreview>>,
}
```

- [ ] **Step 3: Rewrite `new()`, methods, and `reset()`**

Replace the `impl WorkspaceState { ... }` block (everything from `pub fn new()` through `get_tool_payload`) with:

```rust
impl WorkspaceState {
    /// Construct with `localStorage`-hydrated layout mode (best-effort).
    pub fn new() -> Self {
        let hydrated = read_persisted_layout_mode().unwrap_or_default();
        Self {
            mode: RwSignal::new(hydrated),
            tool_payloads: RwSignal::new(HashMap::new()),
            expanded_events: RwSignal::new(HashSet::new()),
            unseen_activity: RwSignal::new(0),
            focus_tool: RwSignal::new(None),
            files_drawer_open: RwSignal::new(false),
            selected_file: RwSignal::new(None),
        }
    }

    /// Toggle between `ChatOnly` and `Split`. Persists to `localStorage`.
    /// Entering Split clears the unseen-activity badge.
    pub fn toggle_layout(&self) {
        let next = self.mode.get_untracked().toggled();
        self.set_layout(next);
    }

    /// Set the layout mode explicitly. Persists to `localStorage`. Entering
    /// Split is treated as "user has seen the activity" → reset the badge.
    pub fn set_layout(&self, mode: LayoutMode) {
        self.mode.set(mode);
        persist_layout_mode(mode);
        if mode == LayoutMode::Split {
            self.unseen_activity.set(0);
        }
    }

    /// Toggle inline expansion of one activity-timeline row.
    pub fn toggle_event(&self, tool_id: &str) {
        self.expanded_events.update(|set| {
            if !set.remove(tool_id) {
                set.insert(tool_id.to_string());
            }
        });
    }

    /// True when the given tool row is expanded inline.
    pub fn is_event_expanded(&self, tool_id: &str) -> bool {
        self.expanded_events.with(|set| set.contains(tool_id))
    }

    /// Chat-chip click: focus a tool row in the timeline, expanding it and
    /// opening Split if needed. Replaces the old `show_tool` single-view.
    pub fn focus_tool_row(&self, _run_id: impl Into<String>, tool_id: impl Into<String>) {
        let tool_id = tool_id.into();
        self.expanded_events.update(|set| {
            set.insert(tool_id.clone());
        });
        self.focus_tool.set(Some(tool_id));
        if self.mode.get_untracked() != LayoutMode::Split {
            self.set_layout(LayoutMode::Split);
        }
    }

    /// Record that a tool started. Bumps the unseen badge only when the
    /// pane is not already open (R5 — never force-open).
    pub fn note_activity(&self) {
        if self.mode.get_untracked() != LayoutMode::Split {
            self.unseen_activity.update(|n| *n += 1);
        }
    }

    /// Reset the pane for a new / switched chat session. Drops inline
    /// expansions, focus, badge, drawer selection, and every captured
    /// payload. Layout mode (the user's pane preference) is preserved.
    pub fn reset(&self) {
        self.tool_payloads.update(|m| m.clear());
        self.expanded_events.update(|s| s.clear());
        self.unseen_activity.set(0);
        self.focus_tool.set(None);
        self.files_drawer_open.set(false);
        self.selected_file.set(None);
    }

    /// Record the input/args of a tool call. Idempotent.
    pub fn record_tool_args(&self, run_id: &str, tool_id: &str, args: serde_json::Value) {
        let key = (run_id.to_string(), tool_id.to_string());
        self.tool_payloads.update(|m| {
            let entry = m.entry(key).or_default();
            entry.args = Some(args);
        });
    }

    /// Record the result of a tool call.
    pub fn record_tool_result(&self, run_id: &str, tool_id: &str, result: serde_json::Value) {
        let key = (run_id.to_string(), tool_id.to_string());
        self.tool_payloads.update(|m| {
            let entry = m.entry(key).or_default();
            entry.result = Some(result);
        });
    }

    /// Lookup the payload for a tool call.
    pub fn get_tool_payload(&self, run_id: &str, tool_id: &str) -> Option<ToolPayload> {
        let key = (run_id.to_string(), tool_id.to_string());
        self.tool_payloads.with(|m| m.get(&key).cloned())
    }
}
```

- [ ] **Step 4: Fix the `reset` test to match new state**

In the `#[cfg(test)] mod tests`, replace the `reset_evicts_payloads_and_content_but_preserves_layout_mode` test body with one that uses the new API (no `show_tool`/`content`):

```rust
    #[test]
    fn reset_evicts_payloads_and_state_but_preserves_layout_mode() {
        let owner = Owner::new();
        owner.set();

        let ws = WorkspaceState {
            mode: RwSignal::new(LayoutMode::Split),
            tool_payloads: RwSignal::new(HashMap::new()),
            expanded_events: RwSignal::new(HashSet::new()),
            unseen_activity: RwSignal::new(0),
            focus_tool: RwSignal::new(None),
            files_drawer_open: RwSignal::new(false),
            selected_file: RwSignal::new(None),
        };
        ws.focus_tool_row("run-1", "tool-a");
        ws.record_tool_args("run-1", "tool-a", serde_json::json!({"q": "x"}));
        ws.record_tool_result("run-1", "tool-a", serde_json::json!({"ok": true}));

        assert!(ws.get_tool_payload("run-1", "tool-a").is_some());
        assert!(ws.is_event_expanded("tool-a"));

        ws.reset();

        assert!(ws.get_tool_payload("run-1", "tool-a").is_none());
        assert!(!ws.is_event_expanded("tool-a"));
        assert_eq!(ws.focus_tool.get_untracked(), None);
        // Layout mode is the user's pane preference — it survives a reset.
        assert_eq!(ws.mode.get_untracked(), LayoutMode::Split);
    }
```

Also add a focused test for `toggle_event` and `note_activity`:

```rust
    #[test]
    fn toggle_event_flips_membership() {
        let owner = Owner::new();
        owner.set();
        let ws = WorkspaceState {
            mode: RwSignal::new(LayoutMode::Split),
            tool_payloads: RwSignal::new(HashMap::new()),
            expanded_events: RwSignal::new(HashSet::new()),
            unseen_activity: RwSignal::new(0),
            focus_tool: RwSignal::new(None),
            files_drawer_open: RwSignal::new(false),
            selected_file: RwSignal::new(None),
        };
        assert!(!ws.is_event_expanded("t1"));
        ws.toggle_event("t1");
        assert!(ws.is_event_expanded("t1"));
        ws.toggle_event("t1");
        assert!(!ws.is_event_expanded("t1"));
    }

    #[test]
    fn note_activity_bumps_badge_only_when_not_split() {
        let owner = Owner::new();
        owner.set();
        let ws = WorkspaceState {
            mode: RwSignal::new(LayoutMode::ChatOnly),
            tool_payloads: RwSignal::new(HashMap::new()),
            expanded_events: RwSignal::new(HashSet::new()),
            unseen_activity: RwSignal::new(0),
            focus_tool: RwSignal::new(None),
            files_drawer_open: RwSignal::new(false),
            selected_file: RwSignal::new(None),
        };
        ws.note_activity();
        ws.note_activity();
        assert_eq!(ws.unseen_activity.get_untracked(), 2);
        // Entering Split clears it.
        ws.set_layout(LayoutMode::Split);
        assert_eq!(ws.unseen_activity.get_untracked(), 0);
        // In Split, further activity does not accrue.
        ws.note_activity();
        assert_eq!(ws.unseen_activity.get_untracked(), 0);
    }
```

> Note: `set_layout(Split)` calls `persist_layout_mode`, which guards on `web_sys::window()` and is a no-op on the non-wasm test target — safe to call in tests.

- [ ] **Step 5: Compile (expect errors in dependent files — that's the next tasks)**

Run: `cargo check -p aleph-panel 2>&1 | head -30`
Expected: errors ONLY in `workspace_panel.rs`, `app.rs`, `views/chat/messages.rs` referencing removed `WorkspaceContent` / `show_tool` / `clear_content`. `layout.rs` itself compiles.

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/state/layout.rs
git commit -m "panel: replace WorkspaceContent single-view with activity-stream state"
```

---

### Task 2: Update the removed-API callers (app.rs, messages.rs)

**Files:**
- Modify: `interfaces/webchat/src/app.rs:285-292`
- Modify: `interfaces/webchat/src/views/chat/messages.rs` (the `show_tool` call site)

- [ ] **Step 1: Replace the chrome-band label in `app.rs`**

In `interfaces/webchat/src/app.rs`, the `<span class="text-text-tertiary/60">` block currently matches on `workspace.content`. Replace its inner `{move || match workspace.content.get() { ... }}` with an unseen-activity-aware label that reuses existing i18n keys:

```rust
                    <span class="text-text-tertiary/60">
                        {move || {
                            let n = workspace.unseen_activity.get();
                            if n > 0 {
                                t_string!(i18n, common.workspace_state_tool).to_string()
                            } else {
                                t_string!(i18n, common.workspace_state_idle).to_string()
                            }
                        }}
                    </span>
```

Remove the now-unused `WorkspaceContent` import in `app.rs` if the compiler flags it (`use ...::WorkspaceContent`). Keep `LayoutMode`.

- [ ] **Step 2: Rename the chip-click call in `messages.rs`**

In `interfaces/webchat/src/views/chat/messages.rs`, find the `ws.show_tool(run_for_click.clone(), tool_id.clone());` call and change it to:

```rust
                            ws.focus_tool_row(run_for_click.clone(), tool_id.clone());
```

- [ ] **Step 3: Compile**

Run: `cargo check -p aleph-panel 2>&1 | head -30`
Expected: errors now ONLY in `workspace_panel.rs` (still references `WorkspaceContent`). `app.rs` and `messages.rs` compile.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/app.rs interfaces/webchat/src/views/chat/messages.rs
git commit -m "panel: point workspace label + chip-click at activity-stream API"
```

---

### Task 3: Rewrite `workspace_panel.rs` as an activity timeline

**Files:**
- Modify: `interfaces/webchat/src/components/workspace_panel.rs`

- [ ] **Step 1: Write a failing test for timeline row derivation**

Add this to the existing `#[cfg(test)] mod tests` in `workspace_panel.rs` (keep the existing `find_tool_entry_*` tests). First add the helper test:

```rust
    #[test]
    fn timeline_rows_flatten_in_document_order_with_run_ids() {
        let owner = Owner::new();
        owner.set();
        let chat = ChatState::new();
        chat.messages.set(vec![
            msg_with_tools("assistant-runA", vec![tool("t1", "read_file"), tool("t2", "search")]),
            msg_with_tools("assistant-runB", vec![tool("t3", "write_file")]),
        ]);
        let rows = timeline_rows(&chat);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ("runA".to_string(), "t1".to_string(), "read_file".to_string()));
        assert_eq!(rows[1], ("runA".to_string(), "t2".to_string(), "search".to_string()));
        assert_eq!(rows[2], ("runB".to_string(), "t3".to_string(), "write_file".to_string()));
    }
```

- [ ] **Step 2: Run it to verify failure**

Run: `cargo test -p aleph-panel --lib timeline_rows_flatten 2>&1 | tail -20`
Expected: FAIL — `cannot find function timeline_rows in this scope`.

- [ ] **Step 3: Rewrite the module body**

Replace the entire contents of `workspace_panel.rs` ABOVE the `#[cfg(test)]` module with:

```rust
//! Workspace pane — the right-side surface that opens when
//! [`LayoutMode::Split`] is active.
//!
//! Renders an **activity timeline**: every tool call in the current
//! session, derived reactively from `ChatState.messages` +
//! `WorkspaceState.tool_payloads`. Rows expand inline to show args/result
//! (file-touching tools therefore reveal their content/diff in place).
//! When no tools have run yet, shows a hero placeholder.

use crate::components::json_viewer::JsonViewer;
use crate::i18n::*;
use crate::state::layout::{LayoutMode, ToolPayload, WorkspaceState};
use crate::views::chat::state::ChatState;
use leptos::prelude::*;

/// Flatten all tool calls across assistant messages into ordered
/// `(run_id, tool_id, tool_name)` rows. The message id is
/// `"assistant-{run_id}"`; strip the prefix to recover the run id used as
/// the `tool_payloads` key.
fn timeline_rows(chat: &ChatState) -> Vec<(String, String, String)> {
    chat.messages
        .get()
        .iter()
        .flat_map(|m| {
            let run = m.id.strip_prefix("assistant-").unwrap_or(&m.id).to_string();
            m.tool_calls
                .iter()
                .map(move |t| (run.clone(), t.tool_id.clone(), t.tool_name.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Best-effort path extraction for a file-touching tool, so the row can
/// show a `📄 path` header. Defensive: tries the known path-bearing arg
/// keys and returns `None` for non-file tools (which then render plain).
fn file_path_of(payload: &Option<ToolPayload>) -> Option<String> {
    let args = payload.as_ref()?.args.as_ref()?;
    for key in ["path", "file_path", "filename"] {
        if let Some(s) = args.get(key).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

/// Workspace pane root. Renders nothing when [`LayoutMode::ChatOnly`].
#[component]
pub fn WorkspacePanel() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();

    view! {
        <Show when=move || workspace.mode.get() == LayoutMode::Split>
            <aside class="aleph-workspace-pane flex flex-col h-full
                           border-l border-border bg-surface-base/40
                           min-w-[280px] basis-[66%] shrink overflow-hidden">
                <div class="flex-1 overflow-y-auto px-4 py-3">
                    <ActivityTimeline />
                </div>
            </aside>
        </Show>
    }
}

/// The reactive activity timeline.
#[component]
fn ActivityTimeline() -> impl IntoView {
    let chat = expect_context::<ChatState>();
    let rows = Memo::new(move |_| timeline_rows(&chat));

    move || {
        let data = rows.get();
        if data.is_empty() {
            view! { <WorkspaceEmptyHero /> }.into_any()
        } else {
            view! {
                <div class="flex flex-col gap-2">
                    {data
                        .into_iter()
                        .map(|(run_id, tool_id, tool_name)| {
                            view! {
                                <ActivityRow
                                    run_id=run_id
                                    tool_id=tool_id
                                    tool_name=tool_name
                                />
                            }
                        })
                        .collect_view()}
                </div>
            }
            .into_any()
        }
    }
}

/// One tool-call row. Click the header to expand args/result inline.
#[component]
fn ActivityRow(run_id: String, tool_id: String, tool_name: String) -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();

    let tid_for_toggle = tool_id.clone();
    let tid_for_expanded = tool_id.clone();
    let tid_for_status = tool_id.clone();
    let run_for_payload = run_id.clone();
    let tid_for_payload = tool_id.clone();

    // Status + duration are looked up live from ChatState so a "running"
    // row flips to "completed" without re-deriving the whole timeline.
    let status = Memo::new(move |_| {
        chat.messages.get().iter().flat_map(|m| m.tool_calls.clone()).find_map(|t| {
            if t.tool_id == tid_for_status {
                Some((t.status.clone(), t.duration_ms))
            } else {
                None
            }
        })
    });

    let payload = Memo::new(move |_| workspace.get_tool_payload(&run_for_payload, &tid_for_payload));
    let expanded = Memo::new(move |_| workspace.is_event_expanded(&tid_for_expanded));

    let path_label = move || file_path_of(&payload.get());

    view! {
        <div class="rounded-md border border-border/60 bg-surface-sunken/40">
            <button
                type="button"
                class="w-full flex items-center gap-2 px-3 py-2 text-left
                       hover:bg-surface-raised/40 transition-colors"
                on:click=move |_| workspace.toggle_event(&tid_for_toggle)
            >
                <span class="text-xs font-mono text-text-secondary">{tool_name.clone()}</span>
                {move || {
                    match status.get() {
                        Some((s, dur)) => {
                            let dur_txt = dur.map(|d| format!(" · {d}ms")).unwrap_or_default();
                            view! {
                                <span class="text-[10px] uppercase tracking-wider text-text-tertiary">
                                    {format!("{s}{dur_txt}")}
                                </span>
                            }
                            .into_any()
                        }
                        None => view! { <span /> }.into_any(),
                    }
                }}
                {move || match path_label() {
                    Some(p) => view! {
                        <span class="ml-auto text-[11px] font-mono text-text-tertiary truncate max-w-[50%]">
                            {format!("📄 {p}")}
                        </span>
                    }
                    .into_any(),
                    None => view! { <span class="ml-auto" /> }.into_any(),
                }}
            </button>
            <Show when=move || expanded.get()>
                <div class="px-3 pb-2">
                    <PayloadBlock payload=payload.get() />
                </div>
            </Show>
        </div>
    }
}

/// Idle placeholder — shown until the first tool call of the session.
#[component]
fn WorkspaceEmptyHero() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="h-full flex flex-col items-center justify-center
                    text-center text-text-tertiary gap-3 py-12 px-6">
            <svg xmlns="http://www.w3.org/2000/svg" class="w-10 h-10 opacity-50"
                 viewBox="0 0 24 24" fill="none" stroke="currentColor"
                 stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="3" width="18" height="18" rx="2"/>
                <line x1="9" y1="3" x2="9" y2="21"/>
                <path d="M14 8h4"/>
                <path d="M14 12h4"/>
                <path d="M14 16h4"/>
            </svg>
            <p class="text-sm font-medium text-text-secondary">{t!(i18n, common.workspace_pane)}</p>
            <p class="text-xs max-w-[24ch] leading-relaxed">
                {t!(i18n, common.workspace_hint)}
            </p>
        </div>
    }
}

/// Args + result hierarchical viewer for a tool call. Hidden when the
/// payload hasn't been captured yet.
#[component]
fn PayloadBlock(payload: Option<ToolPayload>) -> impl IntoView {
    let Some(p) = payload else {
        return view! { <span /> }.into_any();
    };
    view! {
        <div class="flex flex-col gap-2 text-xs">
            <details class="rounded-md border border-border/60 bg-surface-sunken/60" open=true>
                <summary class="px-3 py-1.5 cursor-pointer text-text-tertiary font-mono uppercase tracking-wider">
                    "input"
                </summary>
                <div class="px-3 py-2 overflow-x-auto">
                    {match p.args {
                        Some(v) => view! { <JsonViewer value=v /> }.into_any(),
                        None => view! { <span class="text-text-tertiary italic">"—"</span> }.into_any(),
                    }}
                </div>
            </details>
            <details class="rounded-md border border-border/60 bg-surface-sunken/60" open=true>
                <summary class="px-3 py-1.5 cursor-pointer text-text-tertiary font-mono uppercase tracking-wider">
                    "result"
                </summary>
                <div class="px-3 py-2 overflow-x-auto">
                    {match p.result {
                        Some(v) => view! { <JsonViewer value=v /> }.into_any(),
                        None => view! { <span class="text-text-tertiary italic">"—"</span> }.into_any(),
                    }}
                </div>
            </details>
        </div>
    }
    .into_any()
}
```

> The existing `find_tool_entry` fn and `ToolRendererRegistry` import are dropped — the timeline no longer routes through the registry chip renderer; rows render their own header + `PayloadBlock`. If any other module imported `find_tool_entry` from here, the compiler will flag it (it is `fn`-private, so it will not).

- [ ] **Step 4: Delete the now-stale `find_tool_entry_*` tests**

In the `#[cfg(test)] mod tests`, delete the three `find_tool_entry_*` tests (they reference the removed `find_tool_entry`). Keep `msg_with_tools` and `tool` helpers (used by the new `timeline_rows` test). Ensure the test module still has `use super::*;` and `use crate::views::chat::state::{ChatMessage, ToolCallEntry};` — adjust the imports so only used items remain (`ChatState` is via `super::*`).

- [ ] **Step 5: Run the timeline test**

Run: `cargo test -p aleph-panel --lib timeline_rows_flatten 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: Full panel compile + test**

Run: `cargo check -p aleph-panel 2>&1 | tail -20 && cargo test -p aleph-panel --lib workspace 2>&1 | tail -20`
Expected: compiles; workspace/layout tests pass.

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/components/workspace_panel.rs
git commit -m "panel: render workspace as auto activity timeline with inline expand"
```

---

### Task 4: Auto-populate badge wiring (events + toggle dot)

**Files:**
- Modify: `interfaces/webchat/src/views/chat/events.rs` (tool_call_started branch, ~line 99)
- Modify: `interfaces/webchat/src/components/layout_toggle.rs`

- [ ] **Step 1: Bump activity badge when a tool starts**

In `interfaces/webchat/src/views/chat/events.rs`, inside the `"tool_call_started" =>` arm (after `chat.update_tool(run_id, tool_id, tool_name, "running", None);`), add:

```rust
                        // Surface activity on the toggle when the pane is
                        // closed (R5 — never force-open the Split).
                        workspace.note_activity();
```

- [ ] **Step 2: Add the unseen-activity dot to the toggle**

In `interfaces/webchat/src/components/layout_toggle.rs`, inside the `<button ...>`, after the closing `</svg>`, add a conditional dot. The `workspace` binding already exists in scope. Insert before the button's closing `</button>`:

```rust
            <Show when=move || {
                workspace.mode.get() == LayoutMode::ChatOnly
                    && workspace.unseen_activity.get() > 0
            }>
                <span class="absolute top-0.5 right-0.5 h-2 w-2 rounded-full
                             bg-primary animate-pulse" />
            </Show>
```

Then make the button `relative` so the absolute dot anchors to it — change the button `class=` string's first utility cluster from `flex items-center justify-center` to `relative flex items-center justify-center`.

- [ ] **Step 3: Compile**

Run: `cargo check -p aleph-panel 2>&1 | tail -20`
Expected: compiles clean.

- [ ] **Step 4: Manual smoke (optional, if a dev server is handy)**

Per CLAUDE.md refresh chain: `just wasm` → rebuild `aleph-server` → hot-swap. With the pane closed, send a message that triggers a tool; the toggle should show a pulsing dot; opening Split should clear it and show the tool row.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/chat/events.rs interfaces/webchat/src/components/layout_toggle.rs
git commit -m "panel: surface tool activity on closed pane via toggle dot"
```

---

## Phase 2 — File tree drawer + `fs.read_file`

### Task 5: Core `fs.read_file` RPC

**Files:**
- Modify: `src/gateway/handlers/fs.rs`
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/settings.rs`

- [ ] **Step 1: Write failing tests for the handler**

In `src/gateway/handlers/fs.rs`'s `#[cfg(test)] mod tests`, add (the module already has a `req(...)` helper and a temp-root fixture used by the `list_dir` tests — mirror their setup; the snippet below assumes the same `req` + root helpers those tests use):

```rust
    #[tokio::test]
    async fn read_file_returns_content_in_scope() {
        let (root, _guard) = scoped_root_with_file("hello.txt", "hi there");
        let r = req(
            "fs.read_file",
            json!({ "path": root.join("hello.txt").to_string_lossy() }),
        );
        let resp = handle_read_file(r, test_config(&root)).await;
        let v = resp.result.expect("ok");
        assert_eq!(v["content"], "hi there");
        assert_eq!(v["truncated"], false);
    }

    #[tokio::test]
    async fn read_file_rejects_out_of_scope() {
        let (root, _guard) = scoped_root_with_file("x.txt", "x");
        let outside = std::env::temp_dir().join("definitely-not-in-root.txt");
        let _ = std::fs::write(&outside, "secret");
        let r = req("fs.read_file", json!({ "path": outside.to_string_lossy() }));
        let resp = handle_read_file(r, test_config(&root)).await;
        assert!(resp.error.is_some());
    }
```

> If the existing fs tests do not already expose `scoped_root_with_file` / `test_config`, define them in the test module by adapting the existing `list_dir` test fixtures (they create a temp dir, write a `Config` with `projects.allowed_roots = [root]`, and build `Arc<RwLock<Config>>`). Reuse those exact helpers if present; only add `scoped_root_with_file` as a thin wrapper that also writes a file.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p alephcore --lib gateway::handlers::fs::tests::read_file 2>&1 | tail -20`
Expected: FAIL — `cannot find function handle_read_file`.

- [ ] **Step 3: Add the handler + size cap constant**

In `src/gateway/handlers/fs.rs`, add near `LIST_DIR_CAP`:

```rust
/// Maximum bytes returned by `fs.read_file`. Beyond this the content is
/// truncated and `truncated: true` is set — the preview pane is for
/// reading, not for hauling multi-MB blobs over JSON-RPC.
const READ_FILE_CAP: usize = 512 * 1024;
```

Add the handler after `handle_create_dir` (before the `#[cfg(test)]` module):

```rust
// ─── fs.read_file ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ReadFileParams {
    path: String,
}

pub async fn handle_read_file(request: JsonRpcRequest, config: SharedConfig) -> JsonRpcResponse {
    let params: ReadFileParams = match request
        .params
        .clone()
        .map(serde_json::from_value)
        .transpose()
    {
        Ok(Some(p)) => p,
        Ok(None) => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "params required");
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("invalid params: {e}"),
            );
        }
    };

    let roots = resolve_roots(&*config.read().await);
    if roots.is_empty() {
        return JsonRpcResponse::error(
            request.id,
            OUT_OF_SCOPE,
            "no allowed_roots configured — file reading is disabled",
        );
    }

    let candidate = PathBuf::from(&params.path);
    let canon = match validate_in_scope(&candidate, &roots) {
        Ok(p) => p,
        Err(msg) => return JsonRpcResponse::error(request.id, OUT_OF_SCOPE, &msg),
    };

    let bytes = match std::fs::read(&canon) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return JsonRpcResponse::error(
                request.id,
                NOT_FOUND,
                format!("not found: {}", canon.display()),
            );
        }
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("read failed: {e}"),
            );
        }
    };

    let truncated = bytes.len() > READ_FILE_CAP;
    let slice = if truncated { &bytes[..READ_FILE_CAP] } else { &bytes[..] };
    // Lossy decode: binary / non-UTF8 files still render (replacement
    // chars) rather than erroring — the preview is best-effort.
    let content = String::from_utf8_lossy(slice).into_owned();

    JsonRpcResponse::success(
        request.id,
        json!({
            "path": canon.to_string_lossy(),
            "content": content,
            "truncated": truncated,
        }),
    )
}
```

Also extend the module doc-comment RPC list (top of file) with:

```rust
//! - `fs.read_file { path }`               → `{ path, content, truncated }`
```

- [ ] **Step 4: Run handler tests**

Run: `cargo test -p alephcore --lib gateway::handlers::fs 2>&1 | tail -20`
Expected: PASS (new read_file tests + existing fs tests).

- [ ] **Step 5: Register the handler**

In `src/bin/aleph-server/commands/start/builder/handlers/settings.rs`, after the `fs.create_dir` registration, add:

```rust
    register_handler!(server, "fs.read_file", fs_handlers::handle_read_file, config);
```

And in the `if !daemon { ... }` println block, add a line:

```rust
        println!("  - fs.read_file     : Read a file's text content (size-capped)");
```

- [ ] **Step 6: Compile core**

Run: `cargo check -p alephcore 2>&1 | tail -20`
Expected: compiles clean.

- [ ] **Step 7: Commit**

```bash
git add src/gateway/handlers/fs.rs src/bin/aleph-server/commands/start/builder/handlers/settings.rs
git commit -m "gateway: add scoped fs.read_file RPC for panel file preview"
```

---

### Task 6: Panel `FsApi::read_file` client

**Files:**
- Modify: `interfaces/webchat/src/api/fs.rs`

- [ ] **Step 1: Add the result type + client method**

In `interfaces/webchat/src/api/fs.rs`, add a result struct near `ListDirResult`:

```rust
/// `fs.read_file` response payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadFileResult {
    pub path: String,
    pub content: String,
    pub truncated: bool,
}
```

Add to `impl FsApi`:

```rust
    /// Read a file's text content (server-side, scoped to allowed_roots).
    pub async fn read_file(state: &DashboardState, path: &str) -> Result<ReadFileResult, String> {
        let result = state
            .rpc_call("fs.read_file", serde_json::json!({ "path": path }))
            .await?;
        serde_json::from_value(result).map_err(|e| e.to_string())
    }
```

- [ ] **Step 2: Compile**

Run: `cargo check -p aleph-panel 2>&1 | tail -20`
Expected: compiles clean.

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/api/fs.rs
git commit -m "panel: add FsApi::read_file client"
```

---

### Task 7: Files drawer (tree + preview) in the workspace pane

**Files:**
- Modify: `interfaces/webchat/src/components/workspace_panel.rs`
- Modify: `interfaces/webchat/src/state/layout.rs` (drawer/select helper methods)

- [ ] **Step 1: Add drawer/select methods to `WorkspaceState`**

In `interfaces/webchat/src/state/layout.rs`, add to `impl WorkspaceState`:

```rust
    /// Toggle the bottom files drawer.
    pub fn toggle_files_drawer(&self) {
        self.files_drawer_open.update(|o| *o = !*o);
    }

    /// Set the currently previewed file (None clears the preview pane).
    pub fn select_file(&self, preview: Option<FilePreview>) {
        self.selected_file.set(preview);
    }
```

- [ ] **Step 2: Mount the drawer below the timeline**

In `workspace_panel.rs`, change the `WorkspacePanel` body so the scroll area holds the timeline and a pinned drawer at the bottom:

```rust
                <div class="flex-1 overflow-y-auto px-4 py-3">
                    <ActivityTimeline />
                </div>
                <FilesDrawer />
```

(The `<FilesDrawer />` is a sibling of the scroll div, inside the `<aside>`, so it pins to the bottom.)

- [ ] **Step 3: Implement `FilesDrawer`**

Add to `workspace_panel.rs`. It reuses `DashboardState` for RPC (via `expect_context`), `FsApi` for `list_dir`/`read_file`, and `chat.active_project_root` as the tree root. Add the needed imports at the top of the file:

```rust
use crate::api::fs::{DirEntry, FsApi, ReadFileResult};
use crate::context::DashboardState;
use crate::state::layout::FilePreview;
```

Then the component:

```rust
/// Bottom drawer: collapsible project file tree + read-only preview.
#[component]
fn FilesDrawer() -> impl IntoView {
    let workspace = expect_context::<WorkspaceState>();
    let chat = expect_context::<ChatState>();
    let dashboard = expect_context::<DashboardState>();
    let i18n = use_i18n();

    // Directory listing for the current tree path. Defaults to the active
    // project root; falls back to the first allowed root.
    let entries = RwSignal::new(Vec::<DirEntry>::new());
    let cur_path = RwSignal::new(Option::<String>::None);

    // Load entries whenever the drawer opens (or the project root changes).
    Effect::new(move |_| {
        if !workspace.files_drawer_open.get() {
            return;
        }
        let target = cur_path.get().or_else(|| chat.active_project_root.get());
        let dash = dashboard;
        leptos::task::spawn_local(async move {
            let path = match target {
                Some(p) => p,
                None => match FsApi::allowed_roots(&dash).await {
                    Ok(roots) => match roots.first() {
                        Some(r) => r.path.clone(),
                        None => return,
                    },
                    Err(_) => return,
                },
            };
            if let Ok(listing) = FsApi::list_dir(&dash, &path, false).await {
                cur_path.set(Some(listing.path));
                entries.set(listing.entries);
            }
        });
    });

    let open_file = move |path: String| {
        let dash = dashboard;
        leptos::task::spawn_local(async move {
            if let Ok(ReadFileResult { path, content, truncated }) =
                FsApi::read_file(&dash, &path).await
            {
                workspace.select_file(Some(FilePreview { path, content, truncated }));
            }
        });
    };

    let enter_dir = move |path: String| {
        cur_path.set(Some(path));
    };

    view! {
        <div class="border-t border-border bg-surface-base/60">
            <button
                type="button"
                class="w-full flex items-center gap-2 px-4 py-2 text-left text-xs
                       uppercase tracking-wider text-text-tertiary hover:text-text-secondary"
                on:click=move |_| workspace.toggle_files_drawer()
            >
                <span>{move || t_string!(i18n, common.workspace_files).to_string()}</span>
                <span class="ml-auto">
                    {move || if workspace.files_drawer_open.get() { "▾" } else { "▸" }}
                </span>
            </button>
            <Show when=move || workspace.files_drawer_open.get()>
                <div class="flex max-h-[40vh] border-t border-border/60">
                    // File tree
                    <div class="w-1/3 overflow-y-auto border-r border-border/60 p-2 text-xs">
                        <For
                            each=move || entries.get()
                            key=|e| e.path.clone()
                            children=move |e: DirEntry| {
                                let path = e.path.clone();
                                let is_dir = e.is_dir;
                                let on_click = {
                                    let path = path.clone();
                                    move |_| {
                                        if is_dir {
                                            enter_dir(path.clone());
                                        } else {
                                            open_file(path.clone());
                                        }
                                    }
                                };
                                view! {
                                    <button
                                        type="button"
                                        class="w-full text-left truncate px-1 py-0.5 rounded
                                               hover:bg-surface-raised/50"
                                        on:click=on_click
                                    >
                                        {if e.is_dir { format!("📁 {}", e.name) } else { format!("📄 {}", e.name) }}
                                    </button>
                                }
                            }
                        />
                    </div>
                    // Preview
                    <div class="flex-1 overflow-auto p-2">
                        {move || match workspace.selected_file.get() {
                            Some(f) => view! {
                                <div class="flex flex-col gap-1">
                                    <div class="text-[11px] font-mono text-text-tertiary truncate">
                                        {f.path.clone()}
                                        {if f.truncated { " (truncated)" } else { "" }}
                                    </div>
                                    <pre class="text-xs whitespace-pre-wrap break-words font-mono
                                                text-text-secondary">{f.content.clone()}</pre>
                                </div>
                            }
                            .into_any(),
                            None => view! {
                                <p class="text-xs text-text-tertiary italic">
                                    {t!(i18n, common.workspace_files_hint)}
                                </p>
                            }
                            .into_any(),
                        }}
                    </div>
                </div>
            </Show>
        </div>
    }
}
```

- [ ] **Step 4: Add the three i18n keys**

Find the i18n source for the `common` namespace (search: `grep -rn "workspace_hint" interfaces/webchat`). In the same file(s)/locale(s) where `workspace_hint` is defined, add sibling keys `workspace_files`, `workspace_files_hint` for every locale present. Suggested copy:
- `workspace_files`: en `"Project Files"`, zh `"项目文件"`
- `workspace_files_hint`: en `"Select a file to preview"`, zh `"选择一个文件预览"`

> Match the exact format the locale files use (the project's i18n macro infers keys from these files — a missing key in any locale fails the build, so add to ALL locales).

- [ ] **Step 5: Compile**

Run: `cargo check -p aleph-panel 2>&1 | tail -30`
Expected: compiles clean. If `Effect`/`For`/`spawn_local` paths differ in this Leptos version, fix the import path the compiler suggests (search another component using `For` / `spawn_local`, e.g. `views/agents/files.rs`, and mirror it).

- [ ] **Step 6: Panel tests**

Run: `cargo test -p aleph-panel --lib 2>&1 | tail -20`
Expected: existing tests pass (no new logic test required — drawer is I/O wiring; `read_file` parsing is covered by the `ReadFileResult` round-trip below if you want one).

Optionally add a serde round-trip test in `api/fs.rs` tests:

```rust
    #[test]
    fn read_file_result_round_trips() {
        let v = serde_json::json!({ "path": "/a", "content": "x", "truncated": false });
        let r: ReadFileResult = serde_json::from_value(v).unwrap();
        assert_eq!(r.content, "x");
        assert!(!r.truncated);
    }
```

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/components/workspace_panel.rs interfaces/webchat/src/state/layout.rs interfaces/webchat/src/api/fs.rs
git commit -m "panel: add project file-tree drawer with read-only preview"
```

---

## Final verification

- [ ] **Step 1: Full panel + core check**

Run: `cargo check -p aleph-panel && cargo check -p alephcore`
Expected: both clean.

- [ ] **Step 2: Targeted tests**

Run: `cargo test -p aleph-panel --lib && cargo test -p alephcore --lib gateway::handlers::fs`
Expected: all pass.

- [ ] **Step 3: WASM build + manual smoke**

Per CLAUDE.md refresh chain: `just wasm` → `cargo build --release -p alephcore --bin aleph-server` → hot-swap the running binary. Verify:
1. Closed pane + a tool-using message → pulsing dot on the toggle.
2. Open Split → activity rows auto-listed; dot clears.
3. Click a row → args/result expand inline; file tools show `📄 path`.
4. Expand "Project Files" drawer → tree lists project root; click a file → preview renders; click a folder → tree descends.

- [ ] **Step 4: Run `cargo fmt`**

Run: `cargo fmt -p alephcore && (cd interfaces/webchat && cargo fmt)`
Commit any formatting deltas in the files you touched only.

---

## Self-Review notes (for the implementer)

- **Spec §5.1 inline diff**: delivered as "row expands to args/result via `PayloadBlock`" — file content/old/new appear as data in the JSON viewer. A bespoke side-by-side diff widget is intentionally NOT built (R7 — no diff algorithm; the tool already carries the fields). If a prettier diff is wanted later, it is additive.
- **Spec §5.1 chip path**: `focus_tool_row` keeps the chip click working (expands + scrolls-to via `focus_tool`). A scroll-into-view effect on `focus_tool` is optional polish, not required for correctness.
- **Spec §5.2 / R4**: the only Core addition is `fs.read_file`, pure I/O, same `allowed_roots` gate as siblings.
- **R5**: `note_activity` only accrues while `mode != Split`; entering Split zeroes it. The pane is never force-opened.
