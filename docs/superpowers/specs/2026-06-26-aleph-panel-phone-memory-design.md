# Native iPhone Memory (Vault) — Design

- **Date:** 2026-06-26
- **Topic:** Aleph Panel — FEATURE 4, native iOS phone Memory screen (Vault, v1)
- **Status:** Approved (design); ready for implementation plan
- **Precedes:** `docs/superpowers/plans/2026-06-26-aleph-panel-phone-memory.md`
- **Continues:** FEATURE 3 phone Chat (`2026-06-26-aleph-panel-phone-chat-design.md`),
  iPhone Settings F1/F2 (`2026-06-25-aleph-panel-iphone-settings-*`).

## 1. Context & Motivation

The iOS phone panel (`FormFactor::Phone`, `<640px`) gains one native screen per
tab. **Settings** and **Chat** are done, committed, and deployed. The bottom
`PhoneTabBar` already has a **Memory** item routing to `/memory`, but `app.rs`'s
`MainContent` still renders the **wide** `<MemoryHub />` there for *all* form
factors — so on phone the Memory tab shows the desktop-squished surface, not a
native iOS screen. This feature builds the native phone Memory screen and wires
the form-factor swap, exactly mirroring how FEATURE 3 introduced `PhoneChat`.

Design target: `docs/design-system/aleph-mobile/screens/2-memory.png` +
`screens/exported/Aleph Memory.dc.html`.

## 2. Principles

- **R4 (I/O-only interfaces):** reuse the existing memory data layer; only the
  presentation is phone-specific. No persistence/retrieval logic in the panel.
- **R6 (one core, many channels):** zero core changes, zero new deps; desktop
  bytes unchanged.
- **YAGNI / lean v1:** ship the smallest faithful Vault, defer everything that
  isn't browse + search + read-detail.

## 3. Scope

### In scope (v1)
- Native **Vault** list at `/memory` and a **read-only note detail** at
  `/memory/note`.
- Facet chips over the **note layer**: All Notes / Facts / Feedback / Lessons.
- **Client-side search**: substring-filter the already-loaded note window.
- Client-side pagination via a **"Load more"** button.
- Single **default agent** (no picker).

### Out of scope (deferred to v2 — explicit, no silent scope)
- Graph / WebGL "memory galaxy" toggle (`CanvasView` stays desktop-only).
- **Raw** conversation-log facet (Layer-1, server-paginated).
- Inline note **edit** (`graph.update_note`).
- **Sort** dropdown ("最近更新").
- **Multi-agent** picker.
- **Server-side** note search.

## 4. Reused data layer (no changes except one helper)

From `interfaces/webchat/src/api/memory.rs` and
`interfaces/webchat/src/platform/wide/views/memory/data.rs`:

- `MemoryApi::list_facts(state, agent, Some(NOTE_WINDOW=1000), 0)` → one note
  window, then faceted + paginated **client-side**.
- `MemoryApi::stats(state)` → totals (for any count cross-checks).
- `GraphApi::node_detail(state, agent, path)` → full markdown body + backlinks
  for the detail screen.
- `data.rs`: `MemoryFacet`, `fact_facet`, `bucket_counts` (`[AllNotes, Facts,
  Feedback, Lessons]`), `facet_slice`, `page_slice`, `page_count`, `format_ts`,
  `PAGE_SIZE=50`, `NOTE_WINDOW=1000`, `category_color`, `render_excerpt`.

**New helper (only data-layer addition):**
```
/// Case-insensitive substring filter over a note window by `content`.
/// Empty/whitespace query = passthrough. Pure; unit-tested in data.rs.
pub fn filter_notes(window: &[CompressedFact], query: &str) -> Vec<CompressedFact>
```

The list pipeline is: `window → facet_slice(facet) → filter_notes(query) →
page_slice(page, PAGE_SIZE)`.

## 5. Architecture & wiring

New module `platform/phone/memory/`:

| File | Responsibility |
|------|----------------|
| `mod.rs` | `PhoneMemory` router. Owns shared signals; renders list or detail by route. **No streaming subscription** (request/response). |
| `list.rs` | `PhoneMemoryList` — search field, facet chips, count, cells, "Load more", states. |
| `cell.rs` | `PhoneMemoryCell` — one `.cell` (title + `category · date` sub + type `.badge`). |
| `detail.rs` | `PhoneMemoryDetail` — read-only full note + backlinks. |

Edits to existing files:
- `platform/phone/mod.rs`: add `pub mod memory;`.
- `app.rs` `MainContent`, Memory branch: form-factor swap
  `move || if form_factor == Phone { view!{<PhoneMemory/>} } else { view!{<MemoryHub/>} }`
  (reactive; only one mounts). Mirrors the existing Chat branch.
- `data.rs`: add `filter_notes` + tests.
- `styles/ios.css`: port `.chip`/`.chip-active`, `.badge` +
  `.badge-primary`/`.badge-info`/`.badge-warning`, `.field` from the design HTML.

### Router state (owned by `PhoneMemory`, shared with list/detail via context or props)
- `window: RwSignal<Vec<CompressedFact>>` — loaded note window.
- `loaded: RwSignal<bool>` / `error: RwSignal<Option<String>>` — load state.
- `facet: RwSignal<MemoryFacet>` (default `AllNotes`).
- `query: RwSignal<String>`.
- `page: RwSignal<u32>` (load-more increments; reset on facet/query change).
- `selected: RwSignal<Option<CompressedFact>>` — the note opened in detail.

### Routes
- `PanelMode::from_path` already returns `Memory` for any `/memory*` path
  (`starts_with("/memory")`), so the bottom tab stays active on `/memory/note`. ✓
- `/memory` → `PhoneMemoryList`; `/memory/note` → `PhoneMemoryDetail`.

### Data load (connect-gated — FEATURE 3 cold-boot lesson)
On mount, an `Effect` that only fires once `dashboard.is_connected` is true:
resolve the default agent (reuse `MemoryState.agent_id` / agents default the
wide view uses) → `list_facts(agent, NOTE_WINDOW, 0)` → fill `window`. Before
connect: render "Connecting…"; on error: message + **Retry**.

## 6. Screen — Vault list (`/memory`)

`PhoneShell { title: "Memory" }` (landing, no back). Body:

1. **Search field** (`.field`) bound to `query` (live, case-insensitive).
2. **Facet chips** (`.chip` / `.chip-active`): All Notes / Facts / Feedback /
   Lessons, count badge each from `bucket_counts(window)`.
3. **Count line**: "N 条记忆" for the current facet+filter result. When
   `window.len() == NOTE_WINDOW`, append a truncation notice (no silent caps).
4. **Cells** (`.cell` + `.cell-title` clamped + `.cell-sub` = `category ·
   format_ts(created_at)` + trailing `.badge` colored by facet). Tap → set
   `selected = Some(fact)`, navigate `/memory/note`.
5. **"Load more"** when `page_count(filtered.len()) > page + 1`.

States: connecting → loading → (empty | list) ; error → message + Retry.

**Footgun guard (FEATURE 3):** `PhoneShell` children that mix a static element
with a bare `{move||}` dynamic block drop the static sibling — wrap list body in
a single container element. See `reference-leptos-phoneshell-dynamic-child-footgun`.

## 7. Screen — Note detail (`/memory/note`)

`PhoneShell { title: "Note", back: "/memory" }`. From `selected`:
- category color stripe, full `content`, `path` (mono), date.
- connect-gated `GraphApi::node_detail(agent, path)` → render markdown body
  (`render_excerpt`) + backlinks list. **Read-only** (no edit in v1).
- If `selected == None` (refresh / deep-link on `/memory/note`): redirect to
  `/memory`.

Title is a static `"Note"` (`PhoneShell` takes `&'static str`); same accepted
limitation as the Chat thread's "Conversation" title.

## 8. CSS additions (`styles/ios.css`)

Port from `Aleph Memory.dc.html` (already-present classes reused as-is):
- `.chip`, `.chip-active` — facet pills.
- `.badge`, `.badge-primary`, `.badge-info`, `.badge-warning` — type tags.
- `.field` — search input.

No segmented (`seg`) styles — the Graph/Vault toggle is omitted in v1.

## 9. Testing & verification

- **Unit:** `data.rs::filter_notes` (empty passthrough, case-insensitive match,
  no-match empty) + existing data.rs tests stay green.
- **Build:** `just wasm` (controller-run; minimal `cargo` per project constraint).
- **iOS-sim QA (authoritative):** thin WKWebView shell at **`mobile/ios`**
  (renamed from `mobile/ios-shell`), pointed at a local fresh-dist core `:18790`
  via gitignored `launch-local.sh`. Verify both surfaces:
  - List: facet switching + count, search filters cells live, badges/dates,
    "Load more", Memory tab active.
  - Detail: full note body + backlinks render, `‹ Memory` back works, Memory tab
    active.

## 10. Risks / notes

- **Connect-gating** is mandatory; ungated fetch returns "Not connected" on cold
  boot (FEATURE 3 bug #1).
- **PhoneShell dynamic-child footgun** (FEATURE 3 bug #2): wrap mixed
  static+dynamic children in one element.
- **Pagination DOM bound:** "Load more" keeps the rendered cell count bounded
  (window ≤ 1000; PAGE_SIZE 50).
- **Agent resolution:** reuse the wide view's default-agent path; do not add a
  picker.
