# Memory Canvas Visual Redesign — Design Spec

**Date:** 2026-05-23
**Status:** Brainstormed — awaiting plan
**Author:** Co-designed with user via `superpowers:brainstorming`
**Scope:** Visual + layout overhaul of the memory canvas (`interfaces/webchat/src/views/canvas/` + `canvas_engine/`). MVP focuses on **F1 (visual rewrite) + edge labels** only.

---

## 1. Problem Statement

The current memory canvas suffers from two user-reported defects:

1. **Ugly** — every memory node is rendered as a glowing colored circle with a 10–13px truncated label below. Markdown bodies, tags, and timestamps are invisible until the user clicks into a detail panel. The aesthetic is monotonous: ~100 circles on a starfield, indistinguishable beyond color.
2. **Unreadable** — perfect concentric ring layout (1-hop on `R₁`, 2-hop on `R₂`, orphans on `R_orphan`) plus a single brightly-glowing center creates a **religious-totem** feel. There is no organic flow, no "stars-in-the-sky" sense of structure-in-chaos that knowledge-graph tools like *TheBrain* achieve.

Surface area:
- `canvas_engine/` (~4,300 LOC across 14 files)
- `views/canvas/` (~70 KB across 7 files)
- Already iterated 7+ times (radial-nav → radial-only → elastic-drag → 2026-05-03 knowledge-graph-upgrade)

The fix is **not** a new engine. The fix is:
1. Replace circle+label with rich node cards (DOM overlay over Canvas2D)
2. Replace strict concentric rings with perturbed-radial + Poisson-disk-scattered orphans
3. Replace straight edges with α-gradient Bézier curves layered by hop
4. Move all non-canvas UI (agent / toolbar / breadcrumb / detail) into the shared left sidebar, so the right side is 100% canvas with a collapse button to expand further

---

## 2. Non-Goals (Phase 2 seeds, not in this spec)

- **F2 — JSON Canvas persistence.** Adopt the Obsidian `.canvas` JSON schema as a save/load file format. The redesign uses *some* of its naming conventions (edge `label`, edge `fromSide`/`toSide`) for forward compatibility, but does not implement file I/O.
- **F3 — Agent-driven canvas.** New `canvas_*` tool family that lets the agent create / link / annotate nodes from a chat instruction ("draw the architecture of module X"). Recorded as a Phase 2 seed only; no tools added in this spec.
- **F4 — Live-component nodes** (embedded terminals / charts / code previews). Out of scope — conflicts with R3 (core minimalism) and is better served by chat artifacts.

---

## 3. Architectural Decisions

### 3.1 Stay on Leptos + Canvas2D (option A)

R2 of `CLAUDE.md` requires complex business UI in the Leptos panel. We do **not** introduce React Flow, AntV X6, or any JS-side graph engine. The existing `canvas_engine/` (viewport, drag, navigation, prefetch, tween, interaction) is mature and stays.

### 3.2 DOM overlay for node cards (not Canvas2D text)

Markdown rendering, multi-line truncation, state-driven CSS variants, and crisp typography are all DOM strengths. Canvas2D continues to render:
- Background star-field
- Edges (Bézier + gradient strokes)
- Selection rings, shadows, glows that follow node geometry

Each visible node gets a positioned Leptos component (`NodeCard`); position is updated each rAF tick via `transform: translate3d(x, y, 0)`. Off-viewport nodes are unmounted (viewport culling) to keep the DOM tree small. Drift and spring animations remain in the engine layer — they now write to a Leptos signal that drives the transform.

### 3.3 Sidebar restructure (R2-compliant)

`MemorySidebar` (in `components/mode_sidebar.rs`) is currently a placeholder with one line of hint text. We fill it with the 4 widgets that today live in `RadialCanvasView`'s top stack and right-side detail panel. The right side becomes pure canvas.

### 3.4 No new crates

`pulldown-cmark` (already a dependency) handles excerpt Markdown. All other work is pure Rust + Canvas2D APIs. No new transitive dependencies.

### 3.5 R1–R10 redline check

| Redline | Compliance |
|---|---|
| R2 — UI logic in Leptos | ✅ `NodeCard`, `EdgeLabel`, `MemorySidebar` are all Leptos components |
| R3 — Core minimalism | ✅ No new crates; net LOC expected to decrease |
| R6 — One core many channels | ✅ Frontend-only; core untouched |
| R7 — LLM sovereignty | ✅ No new deterministic decision code displacing LLM judgement |
| R10 — Thin harness | ✅ Not in `src/harness/` |

---

## 4. Layout Restructure (§1b)

### 4.1 Shell convention

The app shell (`app.rs`) is already two-column:

```
<aside class="aleph-sidebar w-64">    256px fixed
  ├─ SidebarBrand                     ℵ logo + theme toggle
  ├─ Mode-specific content            ChatSidebar / MemorySidebar / ...
  └─ NavMenu                          bottom mode switcher
</aside>
<main class="flex-1">                  mode content
```

Width `w-64` (256px) is global; **not** redesigned here.

### 4.2 New `MemorySidebar` content stack (top → bottom)

| Section | Source today | New behavior |
|---|---|---|
| Agent dropdown | `AgentSelectorBar` (right top) | Compact `<select>` with theme |
| Search input | `CanvasToolbar.search` (right top) | Single `<input>`, debounced 200ms |
| Fold threshold slider + visible counts | `CanvasToolbar.fold/counts` | Slider 0..n, "12 nodes · 23 edges" below |
| Breadcrumb path | `Breadcrumb` (right top, conditional) | Inline at top of Detail Panel (small, faded), not a separate row |
| Detail Panel (flex-1) | `DetailPanel` (right side, conditional) | `NodeDetailPanel` — vertical stack fitting 240px content width. When **no node is selected**, shows the **5 most-recently-accessed memories** as clickable list items. |
| Footer | n/a | "12 nodes · 23 edges · ⇧" — collapse button |

### 4.3 Right side = pure canvas

`RadialCanvasView` deletes its outer flex stack. Its body becomes just:

```rust
view! {
    <div class="relative w-full h-full bg-[#080818]">
        <GraphCanvas .../>
        <MiniMapOverlay .../>
    </div>
}
```

### 4.4 Sidebar collapse button

- **Trigger**: ⇧ button at sidebar footer.
- **Mechanism**: toggles `.sidebar-collapsed` on the root `aleph-shell`. CSS: `aside.aleph-sidebar { transform: translateX(-100%); width: 0 }` with 200 ms `ease-out`.
- **Restore**: `Esc` key (only when collapsed — no interference with modals) **or** hover the 8px left-edge strip → semi-transparent ⇨ peek button → click to expand.
- **Persistence**: `localStorage["aleph.sidebar.collapsed"]` survives reload.
- **Scope**: button rendered only in MemorySidebar this cycle; the CSS mechanism is global, so other modes can reuse later.

### 4.5 State sharing

Currently the widgets read state local to `RadialCanvasView`. Moving them into a separate Leptos component requires lifting state. Two options considered:

- **(a)** Provide a `MemoryState` context at `MainContent` level — both `MemorySidebar` and `GraphCanvas` consume it via `use_context::<MemoryState>()`. **Selected.**
- (b) Pass signals as props down the tree. Rejected — too many parameters, ugly threading through 3 layers.

New module `state/memory.rs` exports `MemoryState { agent_id, agents, search_query, fold_threshold, selected_node, focus_id, breadcrumb_entries, recent_visited: VecDeque<NodeId>, sidebar_collapsed }`. Provided at `MainContent` mount.

---

## 5. Node Card Design (§2)

### 5.1 Three render modes

A node renders in **one** of three modes per frame:

| Mode | Used when | Visual |
|---|---|---|
| `FULL` | `hop == 0` OR `hovered` OR `selected` | 280×~150px card: stripe + icon + title + Markdown excerpt + meta footer |
| `MINI` | `hop == 1` | 140×30px pill: dot-icon + 1-line title |
| `DOT` | `hop >= 2` OR `zoom < 0.5` | 10×10px glow circle (hover bumps to 14×14) |

**Upgrade rule**: hover or select bumps the node up exactly one mode (DOT→MINI, MINI→FULL).
**Downgrade rule**: when global `zoom < 0.5`, all nodes force-render as DOT to avoid text blur.

### 5.2 FULL anatomy

```
┌────────────────────────────┐
│ ▭▭▭▭▭ stripe (NodeKind)    │ 3px, gradient
├────────────────────────────┤
│ [⚡] Title (2-line clamp)  │ icon 24×24 + h4 13px/600
├────────────────────────────┤
│ Markdown excerpt           │ 11.5px/400, 3-line clamp
│ (3 lines max, ellipsis)    │
├────────────────────────────┤
│ #tag #tag · 2026-05-23     │ 10px meta, justify-between
└────────────────────────────┘
```

Width 280px (canvas), 240px (sidebar `NodeDetailPanel`). Width is the only difference between canvas-card and sidebar-panel — heights flex.

### 5.3 Design tokens (CSS variables)

```css
--node-bg:           linear-gradient(135deg, #1a1a2e, #16162a);
--node-border:       rgba(255,255,255,0.06);
--text-title:        #f1f5f9;   /* 13px / 600 */
--text-body:         #94a3b8;   /* 11.5px / 400 */
--text-meta:         #64748b;   /* 10px */
--text-code:         #e2e8f0;   /* 10.5px on #0f172a bg */

/* category stripe colors — well-known categories get curated colors,
   any other category gets a deterministic-hash-derived hue (HSL) */
--cat-feedback:      #a78bfa;   /* purple */
--cat-project:       #34d399;   /* green */
--cat-reference:     #60a5fa;   /* blue */
--cat-user:          #fbbf24;   /* yellow */
/* fallback: hsl(hash(category) % 360, 55%, 65%) */

/* shadows / glows */
--shadow-base:       0 8px 32px rgba(0,0,0,0.6);
--glow-hover:        0 0 24px rgba(167,139,250,0.27);
--glow-selected:     0 0 32px rgba(167,139,250,0.53), 0 0 0 2px #a78bfa;
--glow-active:       0 0 32px rgba(252,211,77,0.67);  /* + breath anim */
```

The stripe color is driven by the **existing** `category: String` field on `NoteNodeDto` (no new enum, no server schema change). A small pure function in `views/canvas/node_card.rs` maps category to a CSS variable name or, for unknown categories, returns an HSL string derived from `fxhash(category) % 360`. Well-known categories ("feedback", "project", "reference", "user") get the curated colors above; everything else gets a stable but auto-assigned hue.

```rust
fn category_color(category: &str) -> String {
    match category {
        "feedback"  => "var(--cat-feedback)".into(),
        "project"   => "var(--cat-project)".into(),
        "reference" => "var(--cat-reference)".into(),
        "user"      => "var(--cat-user)".into(),
        other       => {
            let hue = (fxhash::hash32(other.as_bytes()) % 360) as u32;
            format!("hsl({hue}, 55%, 65%)")
        }
    }
}
```

### 5.3a Excerpt sourcing (lazy fetch)

The current `NoteNodeDto` does **not** include note body — only `id`, `name`, `path`, `category`, `tags`, `link_count`. We do **not** extend the server DTO. Instead:

- `hop=0` (active center) — body already loaded via `graph.note.detail` (`NoteDetailResponse.content`). Excerpt = first ~180 chars after stripping front-matter.
- `hop=1, 2` (mini / dot) — no excerpt needed for those render modes.
- **FULL upgrade on hover/select**: lazy-fetch `graph.note.detail` for the upgrading node; reuse the existing `PrefetchCache` (already provides keyed async fetch with in-flight dedupe). Card renders body placeholder for ≤ 1 frame, then real excerpt.

The `PrefetchCache` is hit twice now: once for radial neighbors (existing), once for body content (new). No new cache subsystem; same struct, new key namespace.

### 5.4 Markdown subset (excerpt rendering)

Supported in `excerpt` only (not title):
- `**bold**` → `<strong>`
- `` `code` `` → `<code>`
- `[link](url)` → `<a target=_blank>`
- Hard line break → `<br>`

Everything else (headers, lists, blockquotes, images) is stripped to plain text. Implementation: `pulldown-cmark` with `Options::empty()`, then a whitelist filter on emitted `Event`s.

**XSS**: excerpt content originates from internal memory store (notes the user or agent wrote). No external untrusted input enters this path. Still, we whitelist tag emission and never use `dangerously_inner_html` patterns — Leptos `view! { <span inner_html=html /> }` accepts the filtered string.

### 5.5 State variants (CSS only)

| State | Trigger | Visual delta |
|---|---|---|
| `idle` | default | `--shadow-base` |
| `hover` | `:hover` | `--shadow-base, --glow-hover` |
| `selected` | `data-selected` | `--shadow-base, --glow-selected` |
| `active` | `data-active` (hop=0) | `--shadow-base, --glow-active` + breath animation (`2.5 s ease-in-out infinite`) |

All variants implemented via attribute selectors. No JS state machines.

---

## 6. L2 Organic Layout (§3)

### 6.1 Public API

```rust
// in canvas_engine/layout.rs
pub fn compute_target_positions(
    nodes: &[NoteNodeDto],
    edges: &[EdgeDto],
    center_id: &str,
    viewport: (f32, f32),  // (w_px, h_px)
) -> HashMap<String, Vec2>
```

Pure function. No side effects. Deterministic given identical input.

### 6.2 Algorithm

1. **Bucket by hop** — BFS from `center_id` via `edges`. `one_hop`, `two_hop`, `orphans` sets.
2. **Place center** at origin (world coords `(0, 0)`).
3. **Place 1-hop ring** via `place_perturbed_ring`:
   - Base radius `R₁ = r_one_hop(viewport)` (reuses 2026-05-03 helper)
   - For node `i` of `n`:
     - `angle = (i / n) * 2π + 0.3 * hash_jitter(node_id)` rad (±17° jitter)
     - `radius = R₁ * (0.85 + 0.15 * hash_jitter(node_id))` (±15% radial jitter)
4. **Place 2-hop ring** identically with `R₂ = r_two_hop(viewport)`.
5. **Scatter orphans** via `place_scattered`:
   - Poisson-disk sampling, `min_distance = 2 * dot_radius + 20`
   - Candidate region: viewport minus central 60% (avoid overlapping center + rings)
   - Per orphan: generate 20 candidates from `hash(node_id, attempt_i)`; pick the one with greatest min-distance to all placed nodes
   - If `orphans.len() > 20` → spill to a second outer band with `min_distance × 0.8`

### 6.3 Deterministic jitter

```rust
fn hash_jitter(id: &str) -> f32 {
    // fxhash → i64 → normalize to [-1.0, 1.0]
    let h = fxhash::hash64(id.as_bytes()) as i64;
    let n = (h % 1024) as f32;  // [-512, 512]
    n / 512.0
}
```

Same node id → same offset, every reload. Users build spatial memory ("that one is top-right").

### 6.4 Unit tests (required, in `layout.rs::tests`)

1. `perturbed_ring_is_deterministic` — same input → same `HashMap`
2. `perturbed_ring_avoids_collision` — for `n ≥ 3`, min adjacent angular distance ≥ `(2π / n) * 0.4`
3. `scattered_orphans_avoid_center` — no orphan inside central 60% rect
4. `scattered_orphans_minimum_distance` — pairwise distances ≥ `min_distance`
5. `layout_handles_empty_graph` — 0 / 1 nodes, no edges; no panic
6. `orphan_spill_band_when_count_gt_20` — 25 orphans → 2 distinct radial bands present

### 6.5 References (read at implementation time)

- `/Volumes/TBU4/Github/CodeGraphyV3/src/` — force-graph jitter coefficients (steal numbers, not code)
- `/Volumes/TBU4/Github/tldraw/packages/tldraw/src/lib/` — Poisson-disk reference

---

## 7. Bézier Edges + Labels (§4)

### 7.1 Quadratic Bézier control point

```rust
pub fn edge_control_point(from: Vec2, to: Vec2, sag_coef: f32) -> Vec2 {
    let mid = (from + to) * 0.5;
    let dir = (to - from).normalize();
    let perp = Vec2::new(-dir.y, dir.x);  // 90° CCW
    let sag = (to - from).length() * sag_coef;  // long edges curve more
    mid + perp * sag
}
```

`sag_coef = 0.12` by default. Bidirectional edges flip the sign so they don't visually overlap.

Drawn via `ctx.bezier_curve_to(cp.x, cp.y, cp.x, cp.y, to.x, to.y)` (degenerate cubic, two control points coincide).

### 7.2 α-Gradient stroke

```rust
// pseudo
let grad = ctx.create_linear_gradient(from.x, from.y, to.x, to.y);
grad.add_color_stop(0.00, "rgba(167,139,250,0.00)");
grad.add_color_stop(0.15, &fmt(max_alpha));
grad.add_color_stop(0.85, &fmt(max_alpha));
grad.add_color_stop(1.00, "rgba(167,139,250,0.00)");
```

`max_alpha` by hop:
- 1-hop: `0.85`, `line_width = 1.8`
- 2-hop: `0.55`, `line_width = 1.2`

### 7.3 Hover/Selected highlight

When `hovered_node` or `selected_node` is non-empty:
- Edges adjacent to that node → `max_alpha = 1.0`, `line_width × 1.5`, color shifts to `#fcd34d` (gold)
- Non-adjacent edges → multiply existing `max_alpha × 0.4`
- Two-pass render: dim edges first, bright edges second (z-ordering via draw order)

### 7.4 Edge labels (DOM overlay)

Data model addition (in `canvas_engine/adapter.rs`):

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct NoteLinkDto {
    pub from: String,
    pub to:   String,
    #[serde(default)]
    pub label: Option<String>,   // free-form, matches Obsidian JSON Canvas
    #[serde(default)]
    pub kind:  Option<String>,   // semantic category, e.g. "refers"/"derives"/"follows"/"related"
}
```

Free-form strings (not enums) chosen for: (a) forward compat with Obsidian JSON Canvas, (b) avoids exhausting `match` arms when new relation types appear, (c) keeps server payload tiny if these fields are absent. The set of *recognized* kinds for arrow rendering is a constant in `edge_curve.rs`:

```rust
const DIRECTIONAL_KINDS: &[&str] = &["refers", "derives", "follows"];
```

An edge gets an arrow head iff `link.kind.as_deref().is_some_and(|k| DIRECTIONAL_KINDS.contains(&k))`.

Rendering (Leptos component `EdgeLabel`):
- Position: midpoint of the Bézier, `B(0.5) = 0.25·from + 0.5·cp + 0.25·to`
- Style: pill, `padding 2px 8px; border-radius 6px; background rgba(15,23,42,0.85); color #cbd5e1; font-size 10px`
- Visibility:
  - Hidden by default
  - Fade-in for edges adjacent to hovered/selected node
  - Always hidden when `zoom < 0.7` (illegible)
- Rotation: tangent at midpoint, clamped to [-45°, 45°] so text never inverts

Naming alignment with Obsidian JSON Canvas: `label` field name matches their spec for forward-compatibility if F2 lands later.

### 7.5 Unit tests (in `edge_curve.rs::tests`)

1. `bezier_control_point_deterministic`
2. `control_point_perpendicular_to_edge_axis`
3. `gradient_alpha_by_hop_layer`
4. `edge_label_position_at_t_05` — `B(0.5)` formula
5. `edge_label_rotation_clamped` — never inverts
6. `edge_kind_arrow_only_for_directional_kinds` — kinds in `DIRECTIONAL_KINDS` render arrow; all other kinds (incl. `None`, `"related"`, any unknown string) render plain stroke

---

## 8. File Map

### Modified

| Path | Change |
|---|---|
| `canvas_engine/renderer.rs` | Remove circle-fill node drawing; keep edge / shadow / starfield rendering; remove `set_font/fill_text` for labels |
| `canvas_engine/layout.rs` | Replace `compute_target_positions` body with §6 algorithm; add `place_perturbed_ring`, `place_scattered`; delete the dead orphan-ring constants confirmed in 2026-05-03 sweep |
| `canvas_engine/adapter.rs` | `populate_orphans` → calls `place_scattered`; add `label: Option<String>` + `kind: Option<String>` to `NoteLinkDto`. **No** new fields on `NoteNodeDto`; excerpts are lazy-fetched. |
| `canvas_engine/types.rs` | No new enums — `category` and edge `kind` remain free `String`s |
| `views/canvas/mod.rs` | Strip top stack + right detail; inject `MemoryState` context; right side becomes `<GraphCanvas/> + <MiniMapOverlay/>` only |
| `views/canvas/graph_canvas.rs` | rAF loop now also syncs DOM-overlay transforms |
| `views/canvas/detail_panel.rs` | Rebuild as `NodeDetailPanel` (vertical 240px stack); add "no selection → recent 5" mode |
| `components/mode_sidebar.rs` | Rewrite `MemorySidebar` with the new content stack; add ⇧ collapse button |
| `app.rs` | Provide `MemoryState` context at `MainContent` level; global `Esc` key listener for sidebar collapse |
| `app.css` (or equivalent) | `.aleph-shell.sidebar-collapsed aside.aleph-sidebar { transform: translateX(-100%); width: 0 }` + 8px hover strip + design tokens from §5.3 |

### Added

| Path | Purpose |
|---|---|
| `canvas_engine/edge_curve.rs` | `edge_control_point`, gradient-stroke builder, label position helper |
| `canvas_engine/scatter.rs` | Poisson-disk-like orphan placement |
| `views/canvas/node_card.rs` | Leptos `<NodeCard>` (FULL/MINI/DOT variants) |
| `views/canvas/node_detail_panel.rs` | Sidebar detail panel + "recent 5" empty state |
| `views/canvas/edge_label.rs` | Leptos `<EdgeLabel>` overlay |
| `state/memory.rs` | `MemoryState` struct + context provider |
| `interfaces/webchat/markdown_excerpt.rs` | `pulldown-cmark` excerpt → whitelisted HTML |

### Deleted

- `views/canvas/agent_selector.rs` — content merged into `MemorySidebar`
- `views/canvas/toolbar.rs` — content merged into `MemorySidebar`
- `views/canvas/breadcrumb.rs` — moved as inline element atop `NodeDetailPanel`
- Pre-existing dead constants flagged by 2026-05-03 sweep

Net expected LOC: **roughly flat** (rich Leptos components offset deletion of legacy stack + dead code).

---

## 9. Reference Repositories

Cloned to `/Volumes/TBU4/Github/` for implementation-time inspection:

| Repo | Useful for |
|---|---|
| `obsidianmd/jsoncanvas` | Authoritative JSON Canvas spec — naming for `label`, `fromSide`, `toSide`, `EdgeKind` |
| `Digital-Tvilling/react-jsoncanvas` | TS reference for canvas parsing; Bézier control-point math is portable |
| `dearmydear/code-call-graph-editor` | AntV X6 node/edge styling, color tokens |
| `joesobo/CodeGraphyV3` | Force-graph layout math; jitter coefficients |
| `xiaoiver/infinite-canvas-tutorial` | Pan/zoom matrix transforms |
| `tldraw/tldraw` | Industrial-grade Bézier edges, alignment lines, Poisson-disk |

These are read-only references — no code is vendored. Patterns/numbers/algorithms are reimplemented in Rust to fit Aleph's Leptos+Canvas2D stack.

---

## 10. Performance Targets

| Metric | Target | Why |
|---|---|---|
| Frame budget | < 16.6 ms (60 fps) at 300 visible nodes | Matches current canvas perf |
| Initial render | < 200 ms for 500-node graph | User-perceivable threshold |
| Drift / spring physics | unchanged from current | Reuses existing engine |
| DOM tree size | ≤ visible nodes × 1 (no off-viewport mounting) | Viewport culling enforced |

**Risk**: DOM overlay sync at 300 nodes is unverified. **Mitigation**: a prototype task (Plan Phase 0) must verify 60 fps before any code beyond that point lands. If the prototype fails, fallback is to render node cards as Canvas2D-drawn rounded rects with simplified content (no Markdown), but this is a documented retreat path, not the default.

---

## 11. Verification & Testing Plan

### 11.1 Unit tests (per pure function)

Listed inline in §6.4 and §7.5.

### 11.2 Visual regression

- Phase 0 creates a fixed seed graph fixture: `interfaces/webchat/tests/fixtures/canvas_30nodes.json` (30 nodes / 45 edges / known categories). This fixture does **not** exist yet — Phase 0 generates it from a real memory dump via a one-off script then commits the JSON.
- Test: `cargo test -p aleph-panel --target wasm32-unknown-unknown layout::tests::known_seed_layout` — runs `compute_target_positions` over the fixture, snapshots positions to JSON, diffs against a checked-in baseline. Baseline regenerates only when the test is run with `BLESS_LAYOUT_SNAPSHOTS=1`.

### 11.3 Manual smoke

After implementation:
1. `just dev` — open browser
2. Navigate to memory mode
3. Verify: no top stack, sidebar shows agent/search/fold/path/detail/footer
4. Click a node → detail in sidebar updates
5. Hover node → adjacent edges brighten + labels fade in
6. Click ⇧ → sidebar collapses with 200 ms transition; canvas fills full width
7. Press Esc → sidebar reappears
8. Reload page → collapsed state persists (localStorage)

### 11.4 Acceptance criteria

A merge is acceptable only if:
- All `cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::` pass
- All `cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings` clean
- Manual smoke (11.3) passes on user's machine
- 60 fps at 300 nodes confirmed (Phase 0 prototype gate)
- No regression in existing canvas behaviors: drag, zoom, navigation, prefetch, minimap

---

## 12. Out-of-Scope (recorded for future)

### Phase 2 candidates (separate specs, separate brainstorm sessions)

- **F2 — JSON Canvas persistence.** Read/write `.canvas` files; import from Obsidian; export memory subgraph for sharing.
- **F3 — Agent-driven canvas.** New tool family `canvas.create_node`, `canvas.link`, `canvas.annotate`. Enables conversational graph construction.
- **Force-directed layout (L3).** If L2 still feels static after 1–2 months of real use, evaluate a true Verlet/Barnes-Hut layout. ~200 LOC Rust, no JS deps. Tradeoff: spatial memory loss vs. organic motion.

### Explicit non-goals (will not be done)

- **F4 — Live-component nodes.** Embedded terminals / charts / code previews. Conflicts with R3 (core minimalism). Belongs in chat artifacts, not memory canvas.
- **Switching to React + React Flow.** Violates R2; current Leptos investment is mature.

---

## 13. Build Sequence (hint for the implementation plan)

This spec produces an implementation plan via `superpowers:writing-plans`. The plan is expected to roughly follow:

1. **Phase 0 — Prototype gate**: minimal DOM-overlay-over-canvas spike, 300 nodes, measure fps. Go/no-go.
2. **Phase 1 — Data model**: `NodeKind`, `EdgeKind`, extended DTOs. No visual change yet.
3. **Phase 2 — Layout**: new `place_perturbed_ring` + `place_scattered`. Old visual still works.
4. **Phase 3 — Edges**: Bézier + gradient + hop layering. Labels off.
5. **Phase 4 — Node cards**: `NodeCard` component, three modes, DOM-overlay sync.
6. **Phase 5 — Sidebar restructure**: `MemorySidebar` rewrite + `MemoryState` context.
7. **Phase 6 — Collapse button + Esc + localStorage.**
8. **Phase 7 — Edge labels + hover/selected highlighting.**
9. **Phase 8 — Polish, perf tune, manual smoke, ship.**

Each phase commits independently; the spec's correctness does not depend on phase ordering, but the order minimizes broken intermediate states.

---

## Appendix A — Color & typography reference (single source of truth)

(Tokens are duplicated here for the implementer's quick reference. The Leptos CSS variable names in §5.3 are authoritative.)

Background `#080818` · canvas card `linear-gradient(135deg, #1a1a2e, #16162a)` · title `#f1f5f9 13px/600` · body `#94a3b8 11.5px/400` · meta `#64748b 10px` · kinds {feedback `#a78bfa`, project `#34d399`, reference `#60a5fa`, user `#fbbf24`} · active glow `#fcd34d` · highlight edge gold `#fcd34d`.

---

*End of design spec.*
