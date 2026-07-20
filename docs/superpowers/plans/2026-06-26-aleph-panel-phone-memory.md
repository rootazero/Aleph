# Native iPhone Memory (Vault) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the native iOS phone **Memory (Vault)** screen — a faceted, searchable note list at `/memory` drilling into a read-only note detail at `/memory/note` — and wire it into `app.rs` via a form-factor swap, mirroring the existing phone Chat.

**Architecture:** A new `platform/phone/memory/` module: a router (`mod.rs`) owning `PhoneMemoryState` (note window + UI signals) that connect-gated-loads one `list_facts` window and renders a list (`list.rs` + `cell.rs`) or a detail (`detail.rs`) by route. All persistence/retrieval is reused JSON-RPC (R4); only presentation is new. Vault-only v1 — the Graph/WebGL galaxy and the Raw conversation facet stay desktop-only.

**Tech Stack:** Rust + Leptos (CSR/WASM), `aleph-panel` crate, `leptos_router`, existing `MemoryApi`/`GraphApi`/`AgentsApi`, `views::memory::data` helpers, iOS CSS in `interfaces/webchat/styles/ios.css`.

## Global Constraints

- **R4 / R6:** reuse the data layer; **zero core changes, zero new deps, desktop bytes unchanged.** New code lives only under `interfaces/webchat/src/platform/phone/memory/` plus narrowly-scoped edits to `data.rs`, `phone/mod.rs`, `app.rs`, `ios.css`.
- **Connect-gate every RPC fetch** on `dashboard.is_connected` — an ungated fetch returns `"Not connected"` on cold boot (FEATURE 3 bug #1).
- **PhoneShell children:** wrap any mix of static + dynamic blocks in **one** element; a bare `{move||}` block as a direct component child drops static siblings (FEATURE 3 bug #2 / `reference-leptos-phoneshell-dynamic-child-footgun`).
- **Crate:** `aleph-panel`. **Build gate:** `just wasm` (controller-run; minimal `cargo` per project policy — do not run full test suites).
- **Style:** `cargo fmt`; English commit messages, format `<scope>: <description>`.
- **Facets (v1):** All Notes / Facts / Feedback / Lessons (note layer only); **no** Raw chip. Search is **client-side** substring over the loaded window.

---

### Task 1: `filter_notes` helper + expose the `data` module (TDD)

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/memory/data.rs` (add `filter_notes` + tests)
- Modify: `interfaces/webchat/src/platform/wide/views/memory/mod.rs:12` (`mod data;` → `pub mod data;`)

**Interfaces:**
- Produces: `pub fn filter_notes(window: &[CompressedFact], query: &str) -> Vec<CompressedFact>` — case-insensitive substring filter on `content`; empty/whitespace query = passthrough.
- Produces: `crate::views::memory::data` becomes a public path so the phone module can `use` its helpers.

- [ ] **Step 1: Write the failing tests**

Append inside the existing `#[cfg(test)] mod tests` block in `data.rs` (after the existing tests). The local `fact`/`fact_p` helpers fix `content = "c"`, so add a content-controlling helper:

```rust
    fn fact_content(content: &str) -> CompressedFact {
        CompressedFact {
            id: "i".into(),
            agent_id: "main".into(),
            content: content.into(),
            fact_type: "preference".into(),
            created_at: 0,
            category: "preference".into(),
            path: content.into(),
        }
    }

    #[test]
    fn filter_notes_empty_query_passthrough() {
        let w = vec![fact_content("Alpha"), fact_content("Beta")];
        assert_eq!(filter_notes(&w, "").len(), 2);
        assert_eq!(filter_notes(&w, "   ").len(), 2);
    }

    #[test]
    fn filter_notes_case_insensitive_substring() {
        let w = vec![fact_content("Deploy on 18790"), fact_content("Smoke test first")];
        let r = filter_notes(&w, "SMOKE");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].content, "Smoke test first");
    }

    #[test]
    fn filter_notes_no_match_is_empty() {
        let w = vec![fact_content("Alpha")];
        assert!(filter_notes(&w, "zzz").is_empty());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p aleph-panel filter_notes`
Expected: FAIL — `cannot find function filter_notes in this scope`.

- [ ] **Step 3: Implement `filter_notes`**

Add to `data.rs` (e.g. directly after `facet_slice`):

```rust
/// Case-insensitive substring filter over a note window by `content`.
/// An empty or whitespace-only query is a passthrough (full clone).
#[must_use]
pub fn filter_notes(window: &[CompressedFact], query: &str) -> Vec<CompressedFact> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return window.to_vec();
    }
    window
        .iter()
        .filter(|f| f.content.to_lowercase().contains(&q))
        .cloned()
        .collect()
}
```

- [ ] **Step 4: Expose the module**

In `mod.rs` line 12, change:

```rust
mod data;
```
to:
```rust
pub mod data;
```

(The existing `use data::{...}` inside this file keeps working unchanged.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p aleph-panel filter_notes`
Expected: PASS (3 passed).

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/memory/data.rs \
        interfaces/webchat/src/platform/wide/views/memory/mod.rs
git commit -m "memory: add filter_notes window-search helper; expose data module"
```

---

### Task 2: iOS CSS — chip / badge / field

**Files:**
- Modify: `interfaces/webchat/styles/ios.css` (append rules)

**Interfaces:**
- Produces: `.chip` / `.chip-active`, `.badge` + `.badge-primary` / `.badge-info` / `.badge-warning`, `.field` (+ `:focus` / `::placeholder`) — consumed by Tasks 3–4. All reference design tokens already defined in the panel CSS.

- [ ] **Step 1: Append the rules**

Add to the end of `interfaces/webchat/styles/ios.css` (ported verbatim from `docs/design-system/aleph-mobile/styles/aleph.css`; tokens verified present in the panel CSS):

```css
/* Memory Vault — facet chips, type badges, search field (phone). */
.chip {
  display: inline-flex; align-items: center; gap: 0.35rem;
  min-height: 32px; padding: 0 0.75rem;
  border-radius: var(--radius-full);
  font-size: 0.8125rem; font-weight: 500;
  background: var(--color-surface-raised);
  color: var(--color-text-secondary);
  border: 1px solid var(--color-border);
  cursor: pointer;
}
.chip-active {
  background: var(--color-primary);
  color: var(--color-text-inverse);
  border-color: transparent;
}

.badge {
  display: inline-flex; align-items: center; gap: 0.3rem;
  padding: 0.15rem 0.55rem;
  border-radius: var(--radius-full);
  font-size: 0.6875rem; font-weight: 600;
}
.badge-primary { background: var(--color-primary-subtle); color: var(--color-primary); }
.badge-info    { background: var(--color-info-subtle);    color: var(--color-info); }
.badge-warning { background: var(--color-warning-subtle); color: var(--color-warning); }

.field {
  width: 100%; min-height: 44px; padding: 0.625rem 0.875rem;
  background: var(--color-surface-raised); color: var(--color-text-primary);
  border: 1px solid var(--color-border); border-radius: var(--radius-lg);
  font: inherit; font-size: 0.9375rem;
}
.field::placeholder { color: var(--color-text-tertiary); }
.field:focus { outline: none; border-color: var(--color-border-focus); box-shadow: var(--focus-ring); }
```

- [ ] **Step 2: Verify the classes are present**

Run: `grep -cE '\.chip\b|\.badge\b|\.field\b' interfaces/webchat/styles/ios.css`
Expected: a non-zero count (≥ 3).

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/styles/ios.css
git commit -m "ios.css: add chip/badge/field classes for phone Memory Vault"
```

---

### Task 3: Phone Memory router + Vault list (vertical slice)

Creates the module, the router with `PhoneMemoryState` and the connect-gated loader, the list + cell, and wires the form-factor swap into `app.rs`. The detail route renders a temporary empty stub (replaced in Task 4). End state: on phone, the Memory tab shows a fully working Vault list (search, facet chips, count, cells, Load more) backed by live data.

**Files:**
- Create: `interfaces/webchat/src/platform/phone/memory/mod.rs`
- Create: `interfaces/webchat/src/platform/phone/memory/cell.rs`
- Create: `interfaces/webchat/src/platform/phone/memory/list.rs`
- Modify: `interfaces/webchat/src/platform/phone/mod.rs` (add `pub mod memory;`)
- Modify: `interfaces/webchat/src/app.rs` (Memory branch → form-factor swap + import)

**Interfaces:**
- Consumes (Task 1): `crate::views::memory::data::{MemoryFacet, NOTE_WINDOW, PAGE_SIZE, bucket_counts, facet_slice, filter_notes, fact_facet, format_ts}`.
- Consumes: `crate::api::{CompressedFact, MemoryApi}`, `crate::api::agents::AgentsApi`, `crate::context::DashboardState`, `crate::state::memory::MemoryState`, `crate::platform::phone::shell::PhoneShell`.
- Produces: `pub struct PhoneMemoryState { window, loaded, error, facet, query, page, selected }` (all `RwSignal`, `#[derive(Clone, Copy)]`); `pub fn PhoneMemory()`; `pub fn PhoneMemoryList()`; `pub fn PhoneMemoryCell(fact, on_open)`.

- [ ] **Step 1: Create `cell.rs`**

```rust
//! One Vault note cell: title + "type · date" sub + colored type badge.

use leptos::prelude::*;

use crate::api::CompressedFact;
use crate::views::memory::data::{fact_facet, format_ts, MemoryFacet};

/// (badge label, badge CSS modifier) for a note's facet.
fn badge_for(category: &str) -> (&'static str, &'static str) {
    match fact_facet(category) {
        MemoryFacet::Facts => ("Fact", "badge-primary"),
        MemoryFacet::Feedback => ("Feedback", "badge-info"),
        MemoryFacet::Lessons => ("Lesson", "badge-warning"),
        // fact_facet never returns AllNotes/Raw, but keep the match total.
        _ => ("Note", "badge-info"),
    }
}

#[component]
#[must_use]
pub fn PhoneMemoryCell(fact: CompressedFact, on_open: Callback<CompressedFact>) -> impl IntoView {
    let (label, badge_cls) = badge_for(&fact.category);
    let title = fact.content.clone();
    let sub = format!("{} · {}", label, format_ts(fact.created_at));
    let fact_for_click = fact.clone();
    view! {
        <div class="cell" on:click=move |_| on_open.run(fact_for_click.clone())>
            <div class="cell-body">
                <div class="cell-title" style="font-weight:500;">{title}</div>
                <div class="cell-sub" style="margin-top:2px;">{sub}</div>
            </div>
            <span class=format!("badge {badge_cls}") style="flex:none;">{label}</span>
        </div>
    }
}
```

- [ ] **Step 2: Create `list.rs`**

```rust
//! Phone Vault list (`/memory`): search field, facet chips, count line, note
//! cells, and a "Load more" affordance. Reads the router-owned
//! `PhoneMemoryState`; reuses the memory data layer (R4). Tapping a cell stores
//! the note and drills into `/memory/note`.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::api::CompressedFact;
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use crate::views::memory::data::{
    bucket_counts, facet_slice, filter_notes, page_slice, MemoryFacet, NOTE_WINDOW, PAGE_SIZE,
};

use super::cell::PhoneMemoryCell;
use super::PhoneMemoryState;

/// The four note-layer facet chips (Raw is desktop-only). Index aligns with
/// `bucket_counts` → `[AllNotes, Facts, Feedback, Lessons]`.
const FACETS: [(&str, MemoryFacet); 4] = [
    ("All", MemoryFacet::AllNotes),
    ("Facts", MemoryFacet::Facts),
    ("Feedback", MemoryFacet::Feedback),
    ("Lessons", MemoryFacet::Lessons),
];

#[component]
#[must_use]
pub fn PhoneMemoryList() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let st = expect_context::<PhoneMemoryState>();
    let navigate = use_navigate();

    // Reset pagination whenever the facet or the query changes.
    Effect::new(move || {
        st.facet.get();
        st.query.get();
        st.page.set(0);
    });

    // window → facet_slice → filter_notes  (the faceted, filtered view).
    let visible = move || {
        let w = st.window.get();
        let faceted = facet_slice(&w, st.facet.get());
        filter_notes(&faceted, &st.query.get())
    };
    // Chip badges count the whole window (independent of the search box).
    let counts = move || bucket_counts(&st.window.get());

    view! {
        <PhoneShell title="Memory">
        // Single element child for PhoneShell (mixed static+dynamic siblings
        // must live inside one element — see the PhoneShell footgun note).
        <div style="display:flex; flex-direction:column; gap:12px;">
            <input
                class="field"
                type="text"
                placeholder="搜索记忆…"
                prop:value=move || st.query.get()
                on:input=move |ev| st.query.set(event_target_value(&ev))
            />

            <div class="cc-hide-scroll" style="display:flex; gap:8px; overflow-x:auto; margin:0 -16px; padding:1px 16px;">
                {FACETS.iter().enumerate().map(|(i, (label, f))| {
                    let f = *f;
                    view! {
                        <button
                            class="chip"
                            class:chip-active=move || st.facet.get() == f
                            style="flex:none;"
                            on:click=move |_| st.facet.set(f)
                        >
                            {*label}
                            <span class="tabular-nums" style="opacity:0.7;">
                                {move || counts()[i].to_string()}
                            </span>
                        </button>
                    }
                }).collect_view()}
            </div>

            <div style="display:flex; align-items:center; justify-content:space-between; padding:0 2px;">
                <span style="font-size:12px; font-weight:600; letter-spacing:0.03em; text-transform:uppercase; color:var(--color-text-tertiary);">
                    {move || format!("{} 条记忆", visible().len())}
                </span>
                {move || (st.window.get().len() >= NOTE_WINDOW).then(|| view! {
                    <span style="font-size:11px; color:var(--color-text-tertiary);">"显示前 1000 条"</span>
                })}
            </div>

            {move || {
                if !st.loaded.get() {
                    let label = if dashboard.is_connected.get() { "Loading…" } else { "Connecting…" };
                    return view! { <div class="list-header">{label}</div> }.into_any();
                }
                if let Some(err) = st.error.get() {
                    return view! {
                        <div class="list">
                            <div class="cell"><div class="cell-body"><div class="cell-title">"Couldn't load memories"</div><div class="cell-sub">{err}</div></div></div>
                        </div>
                    }.into_any();
                }
                let items = visible();
                if items.is_empty() {
                    return view! { <div class="list-header">"No memories"</div> }.into_any();
                }
                let total = items.len();
                let shown = (st.page.get() + 1) * PAGE_SIZE; // u32
                let page_items = page_slice(&items, 0, shown);
                view! {
                    <div style="display:flex; flex-direction:column; gap:12px;">
                        <div class="list">
                            {page_items.into_iter().map(|fact: CompressedFact| {
                                let navigate = navigate.clone();
                                let on_open = move |f: CompressedFact| {
                                    st.selected.set(Some(f));
                                    navigate("/memory/note", NavigateOptions::default());
                                };
                                view! { <PhoneMemoryCell fact=fact on_open=Callback::new(on_open)/> }
                            }).collect_view()}
                        </div>
                        {(total > shown as usize).then(|| view! {
                            <button class="chip" style="align-self:center;" on:click=move |_| st.page.update(|p| *p += 1)>
                                "Load more"
                            </button>
                        })}
                    </div>
                }.into_any()
            }}
        </div>
        </PhoneShell>
    }
}
```

- [ ] **Step 3: Create `mod.rs` (router; detail = temporary stub)**

```rust
//! Native iPhone Memory (Vault) screens. Mirrors the phone Chat/Settings
//! pattern: a Vault list landing (`/memory`) drilling into a read-only note
//! detail (`/memory/note`). Reuses the memory data layer (R4); only the
//! presentation is phone-specific. Vault-only v1 — the Graph/galaxy toggle and
//! the Raw conversation facet stay desktop-only.

pub mod cell;
pub mod list;

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_location;

use crate::api::agents::AgentsApi;
use crate::api::{CompressedFact, MemoryApi};
use crate::context::DashboardState;
use crate::state::memory::MemoryState;
use crate::views::memory::data::{MemoryFacet, NOTE_WINDOW};

use self::list::PhoneMemoryList;

/// Router-owned state for the phone Memory screens. Every field is an
/// `RwSignal` (Copy), so the struct is `Copy` and travels via context.
#[derive(Clone, Copy)]
pub struct PhoneMemoryState {
    /// One `list_facts` window; faceted + filtered + paginated client-side.
    pub window: RwSignal<Vec<CompressedFact>>,
    pub loaded: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub facet: RwSignal<MemoryFacet>,
    pub query: RwSignal<String>,
    /// Load-more index; the list shows items `0..(page+1)*PAGE_SIZE`.
    pub page: RwSignal<u32>,
    /// The note opened in the detail screen.
    pub selected: RwSignal<Option<CompressedFact>>,
}

/// Phone Memory router. Owns `PhoneMemoryState`, bootstraps the agent, and
/// connect-gated-loads the note window. Renders the list at `/memory` and the
/// detail at `/memory/note`. No streaming subscription (request/response).
#[component]
#[must_use]
pub fn PhoneMemory() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let mem = expect_context::<MemoryState>();

    let st = PhoneMemoryState {
        window: RwSignal::new(Vec::new()),
        loaded: RwSignal::new(false),
        error: RwSignal::new(None),
        facet: RwSignal::new(MemoryFacet::AllNotes),
        query: RwSignal::new(String::new()),
        page: RwSignal::new(0),
        selected: RwSignal::new(None),
    };
    provide_context(st);

    // Agent bootstrap — honor the server default_id (mirrors the wide Memory
    // view). Idempotent: re-runs until `agents` is non-empty.
    Effect::new(move || {
        if !dashboard.is_connected.get() || !mem.agents.get().is_empty() {
            return;
        }
        spawn_local(async move {
            if let Ok(resp) = AgentsApi::list(&dashboard).await {
                mem.agents.set(resp.agents);
                if mem.agent_id.get_untracked() != resp.default_id {
                    mem.agent_id.set(resp.default_id);
                }
            }
        });
    });

    // Note-window loader — connect-gated (cold-boot lesson) + per-agent.
    Effect::new(move || {
        if dashboard.is_connected.get() {
            let agent = mem.agent_id.get();
            spawn_local(async move {
                st.loaded.set(false);
                st.error.set(None);
                match MemoryApi::list_facts(&dashboard, &agent, Some(NOTE_WINDOW), 0).await {
                    Ok(facts) => st.window.set(facts),
                    Err(e) => st.error.set(Some(e)),
                }
                st.loaded.set(true);
                st.page.set(0);
            });
        } else {
            st.window.set(Vec::new());
            st.loaded.set(false);
        }
    });

    let location = use_location();
    move || {
        if location.pathname.get() == "/memory/note" {
            // Detail screen lands in Task 4; empty stub keeps the slice compiling.
            view! { <div></div> }.into_any()
        } else {
            view! { <PhoneMemoryList/> }.into_any()
        }
    }
}
```

- [ ] **Step 4: Register the module in `phone/mod.rs`**

In `interfaces/webchat/src/platform/phone/mod.rs`, add alongside the existing `pub mod chat;`:

```rust
pub mod memory;
```

- [ ] **Step 5: Wire the form-factor swap in `app.rs`**

Add the import near the existing phone import (`app.rs:44`):

```rust
use crate::platform::phone::memory::PhoneMemory;
```

Replace the Memory branch (currently around `app.rs:399`):

```rust
        <div style:display=move || if mode.get() == PanelMode::Memory { "contents" } else { "none" }>
            <MemoryHub />
        </div>
```
with:
```rust
        <div style:display=move || if mode.get() == PanelMode::Memory { "contents" } else { "none" }>
            {move || if form_factor.form_factor.get() == FormFactor::Phone {
                view! { <PhoneMemory /> }.into_any()
            } else {
                view! { <MemoryHub /> }.into_any()
            }}
        </div>
```

(`form_factor` is already bound in `MainContent`; this mirrors the existing Chat branch.)

- [ ] **Step 6: Build**

Run: `just wasm`
Expected: builds clean (no errors). `cargo fmt` the new files first.

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/platform/phone/memory/ \
        interfaces/webchat/src/platform/phone/mod.rs \
        interfaces/webchat/src/app.rs
git commit -m "phone: add native Memory Vault list + router, wire form-factor swap"
```

---

### Task 4: Read-only note detail screen

Replaces the router's stub with `PhoneMemoryDetail`: full markdown + backlinks for the selected note, fetched via `graph.node_detail`. Redirects to `/memory` if no note is selected (refresh / deep-link).

**Files:**
- Create: `interfaces/webchat/src/platform/phone/memory/detail.rs`
- Modify: `interfaces/webchat/src/platform/phone/memory/mod.rs` (add `pub mod detail;`, import, swap stub)

**Interfaces:**
- Consumes: `super::PhoneMemoryState` (`selected`), `crate::api::graph::GraphApi::node_detail` (→ `NoteDetailResponse { content: String, backlinks: Vec<String> }`), `crate::canvas_engine::category_color::category_color`, `crate::canvas_engine::markdown_excerpt::render_excerpt`, `crate::state::memory::MemoryState`, `crate::context::DashboardState`, `crate::platform::phone::shell::PhoneShell`.
- Produces: `pub fn PhoneMemoryDetail()`.

- [ ] **Step 1: Create `detail.rs`**

```rust
//! Phone note detail (`/memory/note`): read-only full markdown + backlinks for
//! the note selected in the list. Fetches via `graph.node_detail` (R4). If no
//! note is selected (refresh on this route), redirects to `/memory`.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::api::graph::GraphApi;
use crate::canvas_engine::category_color::category_color;
use crate::canvas_engine::markdown_excerpt::render_excerpt;
use crate::context::DashboardState;
use crate::platform::phone::shell::PhoneShell;
use crate::state::memory::MemoryState;

use super::PhoneMemoryState;

#[component]
#[must_use]
pub fn PhoneMemoryDetail() -> impl IntoView {
    let dashboard = expect_context::<DashboardState>();
    let mem = expect_context::<MemoryState>();
    let st = expect_context::<PhoneMemoryState>();
    let navigate = use_navigate();

    let body = RwSignal::new(None::<String>);
    let backlinks = RwSignal::new(Vec::<String>::new());
    let error = RwSignal::new(None::<String>);

    // Redirect when there is no selected note (deep-link / refresh on this route).
    {
        let navigate = navigate.clone();
        Effect::new(move || {
            if st.selected.get().is_none() {
                navigate("/memory", NavigateOptions::default());
            }
        });
    }

    // Fetch full markdown + backlinks once connected, for the selected note.
    Effect::new(move || {
        let Some(fact) = st.selected.get() else { return; };
        if !dashboard.is_connected.get() {
            return;
        }
        let agent = mem.agent_id.get_untracked();
        spawn_local(async move {
            match GraphApi::node_detail(&dashboard, &agent, &fact.path).await {
                Ok(d) => {
                    body.set(Some(d.content));
                    backlinks.set(d.backlinks);
                }
                Err(e) => error.set(Some(e)),
            }
        });
    });

    move || {
        let Some(fact) = st.selected.get() else {
            // The redirect Effect is navigating away; render an empty shell.
            return view! { <PhoneShell title="Note" back="/memory"><div></div></PhoneShell> }.into_any();
        };
        let stripe = category_color(&fact.category);
        let title = fact.content.clone();
        let path = fact.path.clone();
        view! {
            <PhoneShell title="Note" back="/memory">
            <div>
                <div style=format!("height:3px;background:{stripe};border-radius:2px;margin-bottom:10px")></div>
                <h3 style="font-size:16px; font-weight:600; color:var(--color-text-primary); margin:0 0 6px; word-break:break-word;">{title}</h3>
                <div class="mono" style="font-size:12px; color:var(--color-text-tertiary); margin-bottom:14px; word-break:break-all;">{path}</div>

                {move || match body.get() {
                    Some(md) => view! {
                        <div class="node-card-full__excerpt" style="font-size:14px; line-height:1.6; color:var(--color-text-secondary);" inner_html=render_excerpt(&md)></div>
                    }.into_any(),
                    None => view! {
                        <div style="font-size:13px; font-style:italic; color:var(--color-text-tertiary);">"Loading…"</div>
                    }.into_any(),
                }}

                {move || error.get().map(|e| view! {
                    <div style="color:var(--cat-error,#f44336); font-size:13px; margin-top:8px;">{e}</div>
                })}

                {move || {
                    let bl = backlinks.get();
                    (!bl.is_empty()).then(|| view! {
                        <div style="margin-top:18px;">
                            <div style="font-size:10px; text-transform:uppercase; letter-spacing:0.12em; color:var(--color-text-tertiary); margin-bottom:6px;">"Backlinks"</div>
                            <div class="list">
                                {bl.into_iter().map(|b| view! {
                                    <div class="cell"><div class="cell-body"><div class="cell-sub mono" style="word-break:break-all;">{b}</div></div></div>
                                }).collect_view()}
                            </div>
                        </div>
                    })
                }}
            </div>
            </PhoneShell>
        }.into_any()
    }
}
```

- [ ] **Step 2: Swap the stub in `mod.rs`**

Add the submodule declaration next to the others:

```rust
pub mod cell;
pub mod detail;
pub mod list;
```

Add the import next to `use self::list::PhoneMemoryList;`:

```rust
use self::detail::PhoneMemoryDetail;
```

Replace the stub branch in the route switch:

```rust
        if location.pathname.get() == "/memory/note" {
            view! { <div></div> }.into_any()
        } else {
```
with:
```rust
        if location.pathname.get() == "/memory/note" {
            view! { <PhoneMemoryDetail/> }.into_any()
        } else {
```

- [ ] **Step 3: Build**

Run: `just wasm`
Expected: builds clean. `cargo fmt` first.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/platform/phone/memory/detail.rs \
        interfaces/webchat/src/platform/phone/memory/mod.rs
git commit -m "phone: add read-only Memory note detail screen"
```

---

### Task 5: Rebuild dist + iOS-simulator QA

The authoritative verification (FEATURE 3 found two runtime bugs the compile gate and code review both missed). Rebuild dist, run on the iPhone simulator against a local fresh-dist core, fix any runtime issues, then re-verify.

**Files:**
- Modify (build artifact): `interfaces/webchat/dist/aleph_panel.js`, `interfaces/webchat/dist/aleph_panel_bg.wasm` (regenerated by `just wasm`).

- [ ] **Step 1: Rebuild dist**

Run: `just wasm`
Expected: fresh `dist/aleph_panel_bg.wasm` (size changes vs. prior).

- [ ] **Step 2: Launch a local fresh-dist core**

```bash
cd /Volumes/TBU4/Workspace/Aleph
./target/debug/aleph-server -d --bind 127.0.0.1 --port 18790 --log-file /tmp/aleph-local.log start
```
(Build it first if stale: `cargo build --bin aleph-server`. Don't run the desktop GUI app simultaneously — it contends for `:18790` + flock.)

- [ ] **Step 3: Run in the iPhone simulator**

Use the in-repo shell at **`mobile/ios`** with the gitignored `launch-local.sh` (resolves IP + `bootstrap-token` at runtime; see `feedback-ios-panel-testing-via-debian`):

```bash
cd mobile/ios && ./launch-local.sh /memory
```

- [ ] **Step 4: Verify the Vault list**

Screenshot and confirm: title "Memory"; search field filters cells live as you type; the four chips (All/Facts/Feedback/Lessons) switch the list and show counts; the count line reads "N 条记忆"; cells show title + "Type · date" + colored badge; "Load more" appears when results exceed 50; the **Memory** tab is active.

- [ ] **Step 5: Verify the detail screen**

Tap a cell → confirm: drills to `/memory/note` with `‹ Memory` back; full note body renders (markdown) with backlinks when present; back returns to the list; the **Memory** tab stays active on both screens.

- [ ] **Step 6: Fix any runtime issues, rebuild, re-verify**

For each fix: edit the source, `just wasm`, re-run Steps 3–5. Record fixes in the commit message.

- [ ] **Step 7: Commit the rebuilt dist**

```bash
git add interfaces/webchat/dist/aleph_panel.js interfaces/webchat/dist/aleph_panel_bg.wasm
git commit -m "panel: rebuild dist with phone Memory Vault screens"
```

---

## Notes for the implementer

- **Copy handles:** `DashboardState`, `MemoryState`, and `PhoneMemoryState` are all `Copy`; capture them directly into `Effect`/`spawn_local` (no `.clone()`), exactly as the wide Memory view and `PhoneChat` do.
- **No streaming:** unlike `PhoneChat`, the Memory router has **no** `subscribe_*` / `on_cleanup` — it is pure request/response.
- **Window persists across nav:** `PhoneMemoryState` lives in the router above the route switch, so list↔detail and tab switches don't refetch (loader only re-runs on connect/agent change).
- **Deferred to v2 (do not build):** Graph/galaxy toggle, Raw conversation facet, inline note edit, sort dropdown, multi-agent picker, server-side note search.
