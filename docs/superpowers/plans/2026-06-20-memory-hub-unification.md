# Memory Hub Unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fuse the Canvas radial graph (`/memory`) and the Vault table (`/dashboard/memory`) into one "Memory Hub" with an in-place graph⇄table toggle, shared agent/search state, and bidirectional selection linking.

**Architecture:** A thin host `MemoryHub` renders a shared toolbar plus both existing views, switched by CSS `display` (the keep-alive pattern `MainContent` already uses). All shared state lives in the existing `MemoryState` (Copy). The graph node id equals the note path (`entry_to_dto` sets `id = path`), so both link directions use **path** as the shared key.

**Tech Stack:** Rust + Leptos 0.8 (WASM), leptos-i18n (compile-validated keys), leptos_router.

## Global Constraints

- **Frontend only.** No changes under `src/gateway/handlers/` or Rust core. Reuse existing JSON-RPC verbatim.
- **No new dependencies.**
- **Do NOT run `cargo check` / `cargo build` / `cargo test` after a task — commit directly** (standing resource-governance constraint; user override of the skill's run-the-test steps). Unit tests are still written as deliverables for later verification; their "run" steps are explicitly marked SKIP.
- **Branch isolation.** All work in a NEW git worktree branch off `main`; never edit `main` directly.
- **Entropy reduction.** Delete the duplicated agent selectors and the Vault-local search signals this refactor obsoletes.
- Graph node id == note path. Both link directions key on `CompressedFact.path` / `NoteNodeDto.id`.
- Reply language Chinese; code comments English.

---

### Task 0: Create the worktree branch

- [ ] **Step 1: Create an isolated worktree off main**

Use the `superpowers:using-git-worktrees` skill (or `EnterWorktree`) to create a worktree on a new branch `memory-hub-unification`. All subsequent edits happen inside that worktree. Do not touch `main`.

---

### Task 1: MemoryState — view enum, new shared signals, view-param parser

**Files:**
- Modify: `interfaces/webchat/src/state/memory.rs`

**Interfaces:**
- Produces: `pub enum MemoryView { Graph, Table }` (derives `Debug, Clone, Copy, PartialEq, Eq`); `MemoryState.memory_view: RwSignal<MemoryView>`, `MemoryState.highlight_note_id: RwSignal<Option<String>>`, `MemoryState.search_nonce: RwSignal<u32>`; `pub fn parse_view_param(search: &str) -> Option<MemoryView>`.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` in `state/memory.rs`:

```rust
    #[test]
    fn parse_view_param_variants() {
        assert_eq!(parse_view_param("?view=table"), Some(MemoryView::Table));
        assert_eq!(parse_view_param("view=graph"), Some(MemoryView::Graph));
        assert_eq!(parse_view_param("?view=bogus"), None);
        assert_eq!(parse_view_param(""), None);
        assert_eq!(parse_view_param("?foo=1&view=table"), Some(MemoryView::Table));
    }
```

- [ ] **Step 2: Run test to verify it fails** — SKIP per Global Constraints (no cargo). The function does not exist yet, so it would fail to compile.

- [ ] **Step 3: Implement**

Add the enum + parser above the `#[cfg(test)]` module (e.g. just under the `RECENT_VISITED_CAPACITY` const):

```rust
/// Which surface the Memory Hub is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryView {
    Graph,
    Table,
}

/// Parse the `view` query param from a URL search string ("?view=table" or
/// "view=table"). Returns `None` when absent or unknown so callers can leave
/// the current view untouched (manual toggles never write the URL).
#[must_use]
pub fn parse_view_param(search: &str) -> Option<MemoryView> {
    let s = search.strip_prefix('?').unwrap_or(search);
    for pair in s.split('&') {
        if let Some(v) = pair.strip_prefix("view=") {
            return match v {
                "table" => Some(MemoryView::Table),
                "graph" => Some(MemoryView::Graph),
                _ => None,
            };
        }
    }
    None
}
```

Add three fields to the `MemoryState` struct (after `sidebar_collapsed`):

```rust
    pub memory_view: RwSignal<MemoryView>,
    pub highlight_note_id: RwSignal<Option<String>>,
    pub search_nonce: RwSignal<u32>,
```

Initialize them in `new()` (inside the returned `Self { ... }`, after `sidebar_collapsed,`):

```rust
            memory_view: RwSignal::new(MemoryView::Graph),
            highlight_note_id: RwSignal::new(None),
            search_nonce: RwSignal::new(0),
```

- [ ] **Step 4: Run test to verify it passes** — SKIP per Global Constraints. (Provided for later `cargo test -p aleph-panel parse_view_param`.)

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/state/memory.rs
git commit -m "panel: add MemoryView + shared hub signals to MemoryState"
```

---

### Task 2: data.rs — `locate_note` for reverse-link row targeting

**Files:**
- Modify: `interfaces/webchat/src/views/memory/data.rs`

**Interfaces:**
- Consumes: `MemoryFacet`, `fact_facet`, `facet_slice`, `PAGE_SIZE` (same file).
- Produces: `pub fn locate_note(window: &[CompressedFact], path: &str) -> Option<(MemoryFacet, u32)>` — returns the facet to switch to (mapped from the note's category) and the zero-indexed page within that facet's slice; `None` when the path is not in the window.

- [ ] **Step 1: Write the failing test**

Add to `data.rs`'s `#[cfg(test)] mod tests`:

```rust
    fn fact_p(cat: &str, p: &str) -> CompressedFact {
        CompressedFact {
            id: p.into(),
            agent_id: "main".into(),
            content: "c".into(),
            fact_type: cat.into(),
            created_at: 0,
            category: cat.into(),
            path: p.into(),
        }
    }

    #[test]
    fn locate_note_finds_facet_and_page() {
        let mut window: Vec<CompressedFact> =
            (0..60).map(|i| fact_p("preference", &format!("f{i}"))).collect();
        window.push(fact_p("feedback", "fb0"));

        // 56th Facts note (index 55) lands on page 1 (55 / 50).
        assert_eq!(locate_note(&window, "f55"), Some((MemoryFacet::Facts, 1)));
        // First Facts note is on page 0.
        assert_eq!(locate_note(&window, "f0"), Some((MemoryFacet::Facts, 0)));
        // Feedback note maps to the Feedback facet, page 0.
        assert_eq!(locate_note(&window, "fb0"), Some((MemoryFacet::Feedback, 0)));
        // Unknown path → None.
        assert_eq!(locate_note(&window, "missing"), None);
    }
```

- [ ] **Step 2: Run test to verify it fails** — SKIP per Global Constraints (function undefined).

- [ ] **Step 3: Implement** — add above the `#[cfg(test)]` module:

```rust
/// Locate a note by its `path` within the loaded window. Returns the facet to
/// switch to (mapped from the note's category) and the zero-indexed page that
/// holds it within that facet's slice. `None` when the path is not in the
/// window (e.g. it falls outside the NOTE_WINDOW cap) — callers surface a notice.
#[must_use]
pub fn locate_note(window: &[CompressedFact], path: &str) -> Option<(MemoryFacet, u32)> {
    let note = window.iter().find(|f| f.path == path)?;
    let facet = fact_facet(&note.category);
    let slice = facet_slice(window, facet);
    let pos = slice.iter().position(|f| f.path == path)?;
    Some((facet, (pos as u32) / PAGE_SIZE))
}
```

- [ ] **Step 4: Run test to verify it passes** — SKIP per Global Constraints.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/memory/data.rs
git commit -m "panel: add locate_note for reverse-link row targeting"
```

---

### Task 3: i18n keys for the hub

**Files:**
- Modify: `interfaces/webchat/locales/en.json`
- Modify: `interfaces/webchat/locales/zh.json`

**Interfaces:**
- Produces (under the `memory` block): `hub_view_graph`, `hub_view_table`, `view_in_graph`, `view_in_list`, `highlight_not_in_window`. These keys are validated at compile by `t!`/`t_string!` in later tasks — they MUST exist first.

- [ ] **Step 1: Add the five keys to `en.json`** (inside the `"memory": { ... }` object; place after `"batch_clear"`):

```json
    "hub_view_graph": "Graph",
    "hub_view_table": "Table",
    "view_in_graph": "View in graph",
    "view_in_list": "View in list",
    "highlight_not_in_window": "This note is outside the current window — use search to locate it."
```

- [ ] **Step 2: Add the same five keys to `zh.json`** (inside its `"memory"` object, after `"batch_clear"`):

```json
    "hub_view_graph": "图谱",
    "hub_view_table": "列表",
    "view_in_graph": "在图谱中查看",
    "view_in_list": "在列表中查看",
    "highlight_not_in_window": "该笔记不在当前窗口，请用搜索定位。"
```

- [ ] **Step 3: Verify symmetry** (allowed — pure JSON, no cargo):

Run: `python3 -c "import json;e=json.load(open('interfaces/webchat/locales/en.json'));z=json.load(open('interfaces/webchat/locales/zh.json'));print(set(e['memory'])^set(z['memory']))"`
Expected: `set()` (symmetric).

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/locales/en.json interfaces/webchat/locales/zh.json
git commit -m "panel: add i18n keys for memory hub (en/zh)"
```

---

### Task 4: Memory Hub host + shared toolbar

**Files:**
- Create: `interfaces/webchat/src/views/memory_hub/mod.rs`
- Create: `interfaces/webchat/src/views/memory_hub/toolbar.rs`
- Modify: `interfaces/webchat/src/views/mod.rs`

**Interfaces:**
- Consumes: `MemoryState`, `MemoryView`, `parse_view_param` (Task 1); `CanvasView` (`crate::views::canvas`); `Memory` (`crate::views::memory`); `memory.search_placeholder` (existing key), `memory.hub_view_graph`, `memory.hub_view_table` (Task 3); `AgentSummary` fields `id: String`, `name: Option<String>`, `emoji: Option<String>`.
- Produces: `pub fn MemoryHub() -> impl IntoView`.

- [ ] **Step 1: Register the module** — add to `interfaces/webchat/src/views/mod.rs` (keep alphabetical-ish; place after `pub mod memory;`):

```rust
pub mod memory_hub;
```

- [ ] **Step 2: Create `toolbar.rs`:**

```rust
//! Shared Memory Hub toolbar: view toggle (graph⇄table), one search box bound
//! to the shared `search_query`, and the shared agent selector. Pure I/O — it
//! only reads/writes `MemoryState` (R4).

use leptos::prelude::*;

use crate::i18n::{t_string, use_i18n};
use crate::state::memory::{MemoryState, MemoryView};

#[component]
#[must_use]
pub fn MemoryToolbar() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let i18n = use_i18n();
    let is_graph = Memo::new(move |_| mem.memory_view.get() == MemoryView::Graph);

    view! {
        <div class="flex items-center gap-3 px-6 py-3 border-b border-border flex-wrap aleph-content-top">
            // View toggle
            <div class="inline-flex rounded-lg border border-border overflow-hidden">
                <button
                    class=move || if is_graph.get() {
                        "px-3 py-1.5 text-sm bg-primary-subtle text-primary"
                    } else {
                        "px-3 py-1.5 text-sm text-text-tertiary hover:text-text-secondary transition-colors"
                    }
                    on:click=move |_| mem.memory_view.set(MemoryView::Graph)
                >
                    {move || t_string!(i18n, memory.hub_view_graph).to_string()}
                </button>
                <button
                    class=move || if is_graph.get() {
                        "px-3 py-1.5 text-sm text-text-tertiary hover:text-text-secondary transition-colors"
                    } else {
                        "px-3 py-1.5 text-sm bg-primary-subtle text-primary"
                    }
                    on:click=move |_| mem.memory_view.set(MemoryView::Table)
                >
                    {move || t_string!(i18n, memory.hub_view_table).to_string()}
                </button>
            </div>

            // Shared search — Enter bumps `search_nonce` so the table commits its
            // server search; the graph reads `search_query` live for highlight.
            <div class="relative flex-1 min-w-[180px] max-w-md">
                <input
                    type="search"
                    placeholder=t_string!(i18n, memory.search_placeholder)
                    class="w-full px-3 py-1.5 bg-surface-raised border border-border rounded-lg text-sm text-text-primary placeholder:text-text-tertiary focus:outline-none focus:border-primary/50"
                    prop:value=move || mem.search_query.get()
                    on:input=move |ev| mem.search_query.set(event_target_value(&ev))
                    on:keydown=move |ev| { if ev.key() == "Enter" { mem.search_nonce.update(|n| *n += 1); } }
                />
            </div>

            // Shared agent selector
            <select
                class="px-3 py-1.5 bg-surface-raised border border-border rounded-lg text-sm text-text-primary focus:outline-none focus:border-primary/50"
                prop:value=move || mem.agent_id.get()
                on:change=move |ev| mem.agent_id.set(event_target_value(&ev))
            >
                {move || {
                    let current = mem.agent_id.get();
                    let agents = mem.agents.get();
                    if agents.is_empty() {
                        view! { <option value=current.clone()>{current}</option> }.into_any()
                    } else {
                        agents.into_iter().map(|a| {
                            let id = a.id.clone();
                            let label = a.name.as_deref()
                                .map(|n| if let Some(e) = a.emoji.as_deref() { format!("{e} {n}") } else { n.to_string() })
                                .unwrap_or_else(|| a.id.clone());
                            let selected = id == mem.agent_id.get_untracked();
                            view! { <option value=id prop:selected=selected>{label}</option> }
                        }).collect_view().into_any()
                    }
                }}
            </select>
        </div>
    }
}
```

- [ ] **Step 3: Create `mod.rs`:**

```rust
//! Memory Hub — single host that unifies the Canvas graph and the Vault table
//! behind one toolbar and a CSS-`display` view toggle (keep-alive: neither
//! view re-mounts on switch). Shared state lives in `MemoryState`. Pure I/O (R4).

use leptos::prelude::*;
use leptos_router::hooks::use_location;

use crate::state::memory::{parse_view_param, MemoryState, MemoryView};
use crate::views::canvas::CanvasView;
use crate::views::memory::Memory;

mod toolbar;
use toolbar::MemoryToolbar;

#[component]
#[must_use]
pub fn MemoryHub() -> impl IntoView {
    let mem = expect_context::<MemoryState>();
    let location = use_location();

    // Honor `?view=` when the URL query changes — e.g. the /dashboard/memory
    // redirect lands on /memory?view=table. Manual toolbar toggles change
    // `memory_view` WITHOUT touching the URL, so this Effect never fights them
    // (it only re-runs on an actual query-string change, and ignores absence).
    Effect::new(move |_| {
        let search = location.search.get();
        if let Some(v) = parse_view_param(&search) {
            mem.memory_view.set(v);
        }
    });

    let is_graph = Memo::new(move |_| mem.memory_view.get() == MemoryView::Graph);

    view! {
        <div class="flex flex-col h-full min-h-0">
            <MemoryToolbar />
            <div class="flex-1 min-h-0 relative">
                <div
                    class="absolute inset-0"
                    style:display=move || if is_graph.get() { "block" } else { "none" }
                >
                    <CanvasView />
                </div>
                <div
                    class="absolute inset-0 overflow-y-auto"
                    style:display=move || if is_graph.get() { "none" } else { "block" }
                >
                    <Memory />
                </div>
            </div>
        </div>
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/mod.rs interfaces/webchat/src/views/memory_hub/
git commit -m "panel: add MemoryHub host + shared toolbar"
```

---

### Task 5: Route `/memory` to the hub; redirect `/dashboard/memory`

**Files:**
- Modify: `interfaces/webchat/src/app.rs`

**Interfaces:**
- Consumes: `MemoryHub` (Task 4).
- Removes app.rs's direct use of `CanvasView` and `Memory` (now owned by `MemoryHub`).

- [ ] **Step 1: Swap imports.** In `app.rs`, replace the line `use crate::views::canvas::CanvasView;` and the line `use crate::views::memory::Memory;` with:

```rust
use crate::views::memory_hub::MemoryHub;
```

(Delete both old `use` lines — `CanvasView` and `Memory` are no longer referenced directly in `app.rs`.)

- [ ] **Step 2: Add the navigate hook import.** Change the router-hooks import line `use leptos_router::hooks::use_location;` to:

```rust
use leptos_router::hooks::{use_location, use_navigate};
```

- [ ] **Step 3: Render the hub in Memory mode.** In `MainContent`, replace the Memory-mode block:

```rust
        <div style:display=move || if mode.get() == PanelMode::Memory { "contents" } else { "none" }>
            <CanvasView />
        </div>
```

with:

```rust
        <div style:display=move || if mode.get() == PanelMode::Memory { "contents" } else { "none" }>
            <MemoryHub />
        </div>
```

- [ ] **Step 4: Redirect the old Vault route.** In `DashboardRouter`, replace the arm:

```rust
            "/dashboard/memory" => view! { <Memory /> }.into_any(),
```

with:

```rust
            "/dashboard/memory" => view! { <MemoryVaultRedirect /> }.into_any(),
```

- [ ] **Step 5: Add the redirect component** at the end of `app.rs`:

```rust
/// Back-compat redirect: the Vault now lives inside the Memory Hub. Anyone
/// hitting the old `/dashboard/memory` is sent to `/memory?view=table`.
#[component]
fn MemoryVaultRedirect() -> impl IntoView {
    let navigate = use_navigate();
    Effect::new(move |_| {
        navigate("/memory?view=table", leptos_router::NavigateOptions::default());
    });
    ().into_any()
}
```

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/app.rs
git commit -m "panel: route /memory to MemoryHub, redirect legacy /dashboard/memory"
```

---

### Task 6: Trim sidebars (agent/search → toolbar; drop dashboard Vault link)

**Files:**
- Modify: `interfaces/webchat/src/components/mode_sidebar.rs`
- Modify: `interfaces/webchat/src/components/dashboard_sidebar.rs`

**Interfaces:**
- `MemorySidebar` keeps only the Fold slider + `NodeDetailPanel` (agent + search now live in the hub toolbar). `DashboardSidebar` no longer links to `/dashboard/memory`.

- [ ] **Step 1: Trim `MemorySidebar`.** In `mode_sidebar.rs`, inside `fn MemorySidebar`, delete the Agent block and the Search block — i.e. remove these two `<div>` blocks entirely:

  - the `<div class="px-3 pt-3 pb-1.5">` … `</div>` block containing the `"Agent"` label and its `<select>` (the whole agent dropdown);
  - the `<div class="px-3 pb-1.5">` … `</div>` block containing the `"Search"` label and its `<input type="search" …>`.

Leave the Fold block (`<div class="px-3 pb-2">` … fold `<input type="range" …>`) and `<NodeDetailPanel excerpts=excerpts />` intact. The resulting body:

```rust
    view! {
        <div class="flex flex-col h-full">
            <div class="px-3 pb-2">
                <label style="font-size:9.5px;color:var(--color-text-secondary);text-transform:uppercase;letter-spacing:0.05em">
                    "Fold"
                </label>
                <input
                    type="range" min="0" max="10" step="1"
                    class="w-full mt-1 accent-[#a78bfa]"
                    prop:value=move || mem.fold_threshold.get() as i32
                    on:input=move |ev| {
                        if let Ok(v) = event_target_value(&ev).parse::<usize>() {
                            mem.fold_threshold.set(v);
                        }
                    }
                />
            </div>
            <NodeDetailPanel excerpts=excerpts />
        </div>
    }
```

(`mem` is still used by the fold slider, so keep `let mem = expect_context::<MemoryState>();`.)

- [ ] **Step 2: Remove the dashboard Vault link.** In `dashboard_sidebar.rs`, delete the entire `SidebarItem` block for `/dashboard/memory`:

```rust
                <SidebarItem href="/dashboard/memory" label=Signal::derive(move || t_string!(i18n, dashboard.sidebar.memory_vault).to_string()) alert_key="memory.status">
                    <ellipse cx="12" cy="5" rx="9" ry="3" />
                    <path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3" />
                    <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5" />
                </SidebarItem>
```

(Memory is now reached via the bottom NavMenu → Memory section. The `memory.status` alert badge is dropped from the dashboard sub-nav; this is an accepted minor loss — the badge data still loads in `context.rs` and could be re-surfaced on the NavMenu Memory entry in a later pass. The `dashboard.sidebar.memory_vault` i18n key stays defined; leave it.)

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/components/mode_sidebar.rs interfaces/webchat/src/components/dashboard_sidebar.rs
git commit -m "panel: move agent/search to hub toolbar, drop dashboard Vault link"
```

---

### Task 7: Vault wiring — shared search, forward link, reverse-link highlight

**Files:**
- Modify: `interfaces/webchat/src/views/memory/mod.rs`
- Modify: `interfaces/webchat/src/views/memory/facets.rs`

**Interfaces:**
- Consumes: `MemoryState` shared signals (`search_query`, `search_nonce`, `memory_view`, `highlight_note_id`, `selected_node`), `MemoryView` (Task 1); `locate_note` (Task 2); `memory.view_in_graph`, `memory.highlight_not_in_window` (Task 3).
- `FacetBar` gains `on_select: Callback<MemoryFacet>` (page-reset moves to the click site, removing the facet-reset Effect that would clobber the reverse-link page jump). `NotesTable` gains `on_locate: impl Fn(String) + Clone + Send + 'static` and `highlight: Signal<Option<String>>`.

- [ ] **Step 1: Rework `facets.rs`.** Replace the whole file with:

```rust
//! Top facet chips (layer/category switch + count badges) for the memory
//! console. Selection is reported via an `on_select` callback so the parent
//! co-locates page-reset with the click. Pure I/O (R4).

use leptos::prelude::*;

use super::data::MemoryFacet;
use crate::i18n::{t_string, use_i18n};

/// A single facet chip with a count badge.
#[component]
fn FacetChip(
    facet: MemoryFacet,
    active: RwSignal<MemoryFacet>,
    label: String,
    badge: Signal<String>,
    on_select: Callback<MemoryFacet>,
) -> impl IntoView {
    view! {
        <button
            class=move || if active.get() == facet {
                "px-3 py-1.5 text-sm font-medium rounded-lg bg-primary-subtle text-primary"
            } else {
                "px-3 py-1.5 text-sm font-medium rounded-lg text-text-tertiary hover:text-text-secondary transition-colors"
            }
            on:click=move |_| on_select.run(facet)
        >
            {label}
            <span class="ml-1.5 text-[10px] font-mono text-text-tertiary tabular-nums">
                {move || badge.get()}
            </span>
        </button>
    }
}

/// Facet bar. `counts` = `[AllNotes, Facts, Feedback, Lessons]` (note window);
/// `raw_count` = stats total memories (or `None` while unknown). `on_select`
/// fires on every chip click (parent resets the relevant page).
#[component]
pub fn FacetBar(
    active: RwSignal<MemoryFacet>,
    counts: Signal<[usize; 4]>,
    raw_count: Signal<Option<u64>>,
    on_select: Callback<MemoryFacet>,
) -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="flex items-center gap-1 flex-wrap">
            <FacetChip
                facet=MemoryFacet::AllNotes active=active on_select=on_select
                label=t_string!(i18n, memory.facet_all_notes).to_string()
                badge=Signal::derive(move || counts.get()[0].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Facts active=active on_select=on_select
                label=t_string!(i18n, memory.facet_facts).to_string()
                badge=Signal::derive(move || counts.get()[1].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Feedback active=active on_select=on_select
                label=t_string!(i18n, memory.facet_feedback).to_string()
                badge=Signal::derive(move || counts.get()[2].to_string())
            />
            <FacetChip
                facet=MemoryFacet::Lessons active=active on_select=on_select
                label=t_string!(i18n, memory.facet_lessons).to_string()
                badge=Signal::derive(move || counts.get()[3].to_string())
            />
            <span class="mx-1 text-border select-none">"|"</span>
            <FacetChip
                facet=MemoryFacet::Raw active=active on_select=on_select
                label=t_string!(i18n, memory.facet_raw).to_string()
                badge=Signal::derive(move || raw_count.get().map(|c| c.to_string()).unwrap_or_default())
            />
        </div>
    }
}
```

(`AgentFilter` is deleted — the toolbar owns agent selection now — and the `MemoryState` import drops with it.)

- [ ] **Step 2: Update `mod.rs` imports.** Change:

```rust
use crate::state::memory::MemoryState;
```
to:
```rust
use crate::state::memory::{MemoryState, MemoryView};
```

Change:
```rust
use data::{
    bucket_counts, facet_slice, format_ts, page_count, page_slice, MemoryFacet, NOTE_WINDOW,
    PAGE_SIZE,
};
use drawer::{DetailDrawer, DrawerTarget};
use facets::{AgentFilter, FacetBar};
```
to:
```rust
use data::{
    bucket_counts, facet_slice, format_ts, locate_note, page_count, page_slice, MemoryFacet,
    NOTE_WINDOW, PAGE_SIZE,
};
use drawer::{DetailDrawer, DrawerTarget};
use facets::FacetBar;
```

- [ ] **Step 3: Rework the `Memory()` signal/effect block.** In `fn Memory`, replace the local search signal + the facet-reset Effect + `do_search` with the shared-search wiring. Specifically:

Delete the line `let search_query = RwSignal::new(String::new());` (the shared `mem.search_query` replaces it; `applied_query` stays).

Delete the facet-reset Effect:
```rust
    // Reset note page when the facet changes so a switch lands on page 1.
    Effect::new(move || {
        facet.get();
        notes_page.set(0);
    });
```

Delete `do_search`:
```rust
    let do_search = move || {
        applied_query.set(search_query.get());
        raw_page.set(0);
    };
```

Add, right after the raw-loader Effect, the shared-search commit Effect + the highlight state + the reverse-link Effect:

```rust
    // Shared search box (in the hub toolbar) writes `mem.search_query` live and
    // bumps `mem.search_nonce` on Enter. Commit that into the Raw search here;
    // a non-empty query also switches to the Raw facet so results are visible.
    Effect::new(move || {
        mem.search_nonce.get(); // subscribe to submit pulses
        let q = mem.search_query.get_untracked();
        applied_query.set(q.clone());
        raw_page.set(0);
        if !q.is_empty() {
            facet.set(MemoryFacet::Raw);
        }
    });

    // Reverse link (graph → list): the node detail panel sets
    // `mem.highlight_note_id`; jump to the note's facet+page, highlight the row,
    // and open its drawer. Outside the loaded window → inline notice.
    let highlight_id = RwSignal::new(None::<String>);
    let highlight_missing = RwSignal::new(false);
    Effect::new(move || {
        let Some(path) = mem.highlight_note_id.get() else {
            return;
        };
        if !notes_loaded.get() {
            return; // wait for the window; re-runs when it loads
        }
        let window = notes_window.get();
        match locate_note(&window, &path) {
            Some((f, pg)) => {
                facet.set(f);
                notes_page.set(pg);
                highlight_id.set(Some(path.clone()));
                highlight_missing.set(false);
                if let Some(found) = window.into_iter().find(|x| x.path == path) {
                    drawer_target.set(Some(DrawerTarget::Note(found)));
                }
            }
            None => {
                highlight_missing.set(true);
                highlight_id.set(None);
            }
        }
        mem.highlight_note_id.set(None); // consume so re-selecting re-triggers
    });
```

- [ ] **Step 4: Wire the facet-bar callback + remove the header agent filter + remove the in-content search input.** In the `view!`:

Remove `<AgentFilter />` from the `<header>` (delete that line).

Replace the facet-bar row:
```rust
            // Facet bar + (raw) search
            <div class="flex items-center justify-between gap-3 flex-wrap border-b border-border pb-2">
                <FacetBar active=facet counts=counts raw_count=raw_total />
                {move || {
                    if facet.get() == MemoryFacet::Raw {
                        view! {
                            <div class="relative group">
                                ... search input ...
                            </div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}
            </div>
```
with (drop the whole search-input closure; search lives in the toolbar now):
```rust
            // Facet bar (search lives in the hub toolbar).
            <div class="flex items-center justify-between gap-3 flex-wrap border-b border-border pb-2">
                <FacetBar
                    active=facet
                    counts=counts
                    raw_count=raw_total
                    on_select=Callback::new(move |f: MemoryFacet| { facet.set(f); notes_page.set(0); })
                />
            </div>

            // Reverse-link notice when the highlighted note is outside the window.
            {move || highlight_missing.get().then(|| view! {
                <p class="text-xs text-warning italic">{t!(i18n, memory.highlight_not_in_window)}</p>
            })}
```

- [ ] **Step 5: Pass forward-link + highlight into `NotesTable`.** Replace the notes branch of the content closure:

```rust
                    view! {
                        <NotesTable
                            window=notes_window
                            facet=facet
                            page=notes_page
                            loaded=notes_loaded
                            connected=connected
                            on_open=move |fact| drawer_target.set(Some(DrawerTarget::Note(fact)))
                        />
```
with (add `on_locate` + `highlight`):
```rust
                    view! {
                        <NotesTable
                            window=notes_window
                            facet=facet
                            page=notes_page
                            loaded=notes_loaded
                            connected=connected
                            highlight=Signal::derive(move || highlight_id.get())
                            on_open=move |fact| drawer_target.set(Some(DrawerTarget::Note(fact)))
                            on_locate=move |path: String| {
                                mem.selected_node.set(Some(path));
                                mem.memory_view.set(MemoryView::Graph);
                            }
                        />
```

- [ ] **Step 6: Extend the `NotesTable` component.** Change its signature:

```rust
#[component]
fn NotesTable(
    window: RwSignal<Vec<CompressedFact>>,
    facet: RwSignal<MemoryFacet>,
    page: RwSignal<u32>,
    loaded: RwSignal<bool>,
    connected: RwSignal<bool>,
    on_open: impl Fn(CompressedFact) + Clone + Send + 'static,
) -> impl IntoView {
```
to:
```rust
#[component]
fn NotesTable(
    window: RwSignal<Vec<CompressedFact>>,
    facet: RwSignal<MemoryFacet>,
    page: RwSignal<u32>,
    loaded: RwSignal<bool>,
    connected: RwSignal<bool>,
    highlight: Signal<Option<String>>,
    on_open: impl Fn(CompressedFact) + Clone + Send + 'static,
    on_locate: impl Fn(String) + Clone + Send + 'static,
) -> impl IntoView {
```

Inside the row builder, clone `on_locate` alongside `on_open`. Replace the per-row construction:

```rust
                        let on_open = on_open.clone();
                        let fact_for_click = fact.clone();
                        view! {
                            <tr class="group hover:bg-surface-sunken transition-colors cursor-pointer" on:click=move |_| on_open(fact_for_click.clone())>
                                <td class="p-4 pl-8">
                                    <div class="text-sm font-medium text-text-primary line-clamp-2 group-hover:line-clamp-none transition-all">{content}</div>
                                    <div class="text-xs text-text-tertiary mt-0.5 font-mono">{path}</div>
                                </td>
```
with (capture `path` for locate + highlight, add the graph button + highlight ring):

```rust
                        let on_open = on_open.clone();
                        let on_locate = on_locate.clone();
                        let fact_for_click = fact.clone();
                        let path_for_locate = path.clone();
                        let path_for_highlight = path.clone();
                        let i18n_row = i18n;
                        let is_hl = Signal::derive(move || highlight.get().as_deref() == Some(path_for_highlight.as_str()));
                        view! {
                            <tr
                                class=move || if is_hl.get() {
                                    "group bg-primary-subtle ring-1 ring-primary/40 transition-colors cursor-pointer"
                                } else {
                                    "group hover:bg-surface-sunken transition-colors cursor-pointer"
                                }
                                on:click=move |_| on_open(fact_for_click.clone())
                            >
                                <td class="p-4 pl-8">
                                    <div class="flex items-start justify-between gap-2">
                                        <div class="min-w-0">
                                            <div class="text-sm font-medium text-text-primary line-clamp-2 group-hover:line-clamp-none transition-all">{content}</div>
                                            <div class="text-xs text-text-tertiary mt-0.5 font-mono">{path}</div>
                                        </div>
                                        <button
                                            class="flex-shrink-0 p-1 rounded text-text-tertiary opacity-0 group-hover:opacity-100 hover:text-primary transition-all"
                                            title=t_string!(i18n_row, memory.view_in_graph)
                                            on:click=move |ev| { ev.stop_propagation(); on_locate(path_for_locate.clone()); }
                                        >
                                            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                                                <circle cx="5" cy="6" r="2" /><circle cx="19" cy="6" r="2" /><circle cx="12" cy="18" r="2" />
                                                <line x1="6.6" y1="7.4" x2="10.6" y2="16.4" /><line x1="17.4" y1="7.4" x2="13.4" y2="16.4" /><line x1="7" y1="6" x2="17" y2="6" />
                                            </svg>
                                        </button>
                                    </div>
                                </td>
```

(`path` is consumed by the markup, so the two `path_for_*` clones are taken before it is moved. `i18n` is `Copy` — `let i18n_row = i18n;` is free and avoids moving the outer binding into per-row closures.)

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/views/memory/mod.rs interfaces/webchat/src/views/memory/facets.rs
git commit -m "panel: wire Vault to shared search + forward/reverse hub links"
```

---

### Task 8: Reverse-link button in the canvas node detail panel

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/node_detail_panel.rs`

**Interfaces:**
- Consumes: `MemoryState.memory_view`, `MemoryState.highlight_note_id`, `MemoryView` (Task 1); `memory.view_in_list` (Task 3). `excerpt.id` == note path.

- [ ] **Step 1: Import `MemoryView`.** Change:

```rust
use crate::state::memory::MemoryState;
```
to:
```rust
use crate::state::memory::{MemoryState, MemoryView};
```

- [ ] **Step 2: Keep a path copy for the reverse link.** In `fn DetailFor`, just after `let node_id = excerpt.id.clone();`, add:

```rust
    let node_id_for_list = excerpt.id.clone();
```

- [ ] **Step 3: Add the "View in list" button.** In `DetailFor`'s `view!`, immediately after the `<h3 ...>{title}</h3>` line, insert:

```rust
            <button
                class="node-detail-btn"
                style="margin:0 0 8px"
                on:click=move |_| {
                    mem.highlight_note_id.set(Some(node_id_for_list.clone()));
                    mem.memory_view.set(MemoryView::Table);
                }
            >
                {t!(i18n, memory.view_in_list)}
            </button>
```

(`mem` and `i18n` are already in scope in `DetailFor`. `node_id_for_list` moves into this `on:click` closure; the existing `node_id` binding still moves into `save_edit` separately.)

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/canvas/node_detail_panel.rs
git commit -m "panel: add 'view in list' reverse link to node detail panel"
```

---

## Final Verification (per constraints — no cargo)

- [ ] **Step 1:** `git log --oneline main..HEAD` — expect Tasks 1–8 as separate commits.
- [ ] **Step 2:** `python3 -c "import json;e=json.load(open('interfaces/webchat/locales/en.json'));z=json.load(open('interfaces/webchat/locales/zh.json'));print('sym', set(e['memory'])^set(z['memory']))"` — expect `sym set()`.
- [ ] **Step 3:** `grep -rn "/dashboard/memory" interfaces/webchat/src` — expect only the redirect arm in `app.rs` and the doc-comment in `views/memory/mod.rs` (no live `SidebarItem`).
- [ ] **Step 4:** `grep -rn "AgentFilter" interfaces/webchat/src` — expect no matches (deleted).
- [ ] **Step 5:** Report completion. Do NOT run cargo. Real compile/visual verification (`just wasm` + binary rebuild) is the user's call, outside this plan.

## Notes for the implementer

- **Why no cargo:** the project enforces a resource-governance constraint — commit without compiling. Correctness here rests on reusing already-typed APIs and mirroring existing patterns. The two pure-logic tests (Tasks 1–2) are runnable later with `cargo test -p aleph-panel`.
- **Key identity:** graph node id == note path (`entry_to_dto` in `src/gateway/handlers/graph.rs`). Every link uses `path`. Do not switch to `CompressedFact.id`.
- **Canvas needs no edits for the forward link** — it already reacts to `mem.selected_node`. Only `node_detail_panel.rs` gains the reverse button.
- **Search semantics:** graph reads `mem.search_query` live (highlight); table commits on Enter via `mem.search_nonce`. Empty query = browse.
```
