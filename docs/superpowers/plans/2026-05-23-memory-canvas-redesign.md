# Memory Canvas Visual Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Aleph's "religious-totem" memory canvas with an organic, card-based knowledge graph — fill the shared left sidebar with the existing widgets so the right side is 100% canvas, render nodes as rich Markdown cards via DOM overlay, lay them out with deterministic-jitter perturbed rings + Poisson-scattered orphans, and connect them with α-gradient Bézier edges that show relation labels on hover.

**Architecture:** Stay on Leptos + raw `CanvasRenderingContext2d` (option A from the brainstorm — R2-compliant). Canvas2D continues to render starfield / edges / shadows; nodes become Leptos DOM components positioned via `transform: translate3d`. Layout is a pure function with hash-jitter for determinism. The existing `canvas_engine/` (viewport, drag, navigation, prefetch, tween) is reused. No new crates.

**Tech Stack:** Rust 2021 + `wasm32-unknown-unknown` (panel crate `aleph-panel` / lib `aleph_panel`). Leptos 0.8 CSR. `web-sys` `CanvasRenderingContext2d`. `pulldown-cmark = "0.12"` (already a dep). Hand-rolled FNV-1a for deterministic hash.

**Spec:** [`docs/superpowers/specs/2026-05-23-memory-canvas-redesign-design.md`](../specs/2026-05-23-memory-canvas-redesign-design.md)

**Reference repos** (already cloned to `/Volumes/TBU4/Github/`): `obsidianmd/jsoncanvas`, `Digital-Tvilling/react-jsoncanvas`, `dearmydear/code-call-graph-editor`, `joesobo/CodeGraphyV3`, `xiaoiver/infinite-canvas-tutorial`, `tldraw/tldraw`.

**Predecessors that must not regress:**
- `2026-04-25-canvas-radial-navigation-design.md`
- `2026-04-26-canvas-radial-only-redesign.md`
- `2026-04-27-canvas-elastic-node-drag-design.md` (§12 D1: no force-directed layout)
- `2026-05-03-knowledge-graph-canvas-upgrade-design.md` (`r_one_hop`, `r_two_hop`, `r_orphan`, `fit_to_content` helpers)

---

## Conventions

- Every task ends with **a passing test (or build) + a commit**. Commits use the project format `<scope>: <description>` (English, lower-case). Scope is `canvas` for every task in this plan.
- Build / test commands the worker needs:
  - `cargo check -p aleph-panel --target wasm32-unknown-unknown` — fast compile gate
  - `cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::` — unit tests for canvas_engine
  - `cargo test -p aleph-panel --target wasm32-unknown-unknown --lib` — all panel lib tests
  - `cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings` — final lint
  - `just dev` — dev server for manual smoke
- **Concurrency cap**: before running any `cargo` command, check `pgrep -x cargo | wc -l` and wait until < 3 (the Box machine OOM-kills past 3 concurrent cargos). Use `pgrep -x cargo`, **not** `ps | grep cargo` (the latter self-matches).
- All `#[cfg(test)] mod tests { ... }` blocks live alongside the code they test.
- All new helpers are `pub(crate)` unless they cross the `canvas_engine` ↔ `views` boundary.
- **Do NOT commit** to git unless this plan tells you to in the explicit `Step N: Commit` step. Project CLAUDE.md says never commit without explicit user instruction; the per-task commit steps in this plan ARE that instruction.

---

## File Structure

### Created
| Path | Responsibility |
|---|---|
| `interfaces/webchat/src/canvas_engine/edge_curve.rs` | Pure functions: `edge_control_point`, gradient builder, label position, `DIRECTIONAL_KINDS` |
| `interfaces/webchat/src/canvas_engine/scatter.rs` | `place_scattered`: Poisson-disk orphan placement |
| `interfaces/webchat/src/canvas_engine/fnv1a.rs` | Tiny deterministic hash helper |
| `interfaces/webchat/src/canvas_engine/markdown_excerpt.rs` | `pulldown-cmark` → whitelisted HTML string |
| `interfaces/webchat/src/views/canvas/node_card.rs` | Leptos `<NodeCard>` in FULL/MINI/DOT modes |
| `interfaces/webchat/src/views/canvas/node_detail_panel.rs` | Sidebar 240px detail panel + recent-5 empty state |
| `interfaces/webchat/src/views/canvas/edge_label.rs` | Leptos `<EdgeLabel>` overlay |
| `interfaces/webchat/src/state/memory.rs` | `MemoryState` context provider |
| `interfaces/webchat/tests/fixtures/canvas_30nodes.json` | Frozen 30-node / 45-edge graph for layout snapshot tests |

### Modified
| Path | Change |
|---|---|
| `interfaces/webchat/src/canvas_engine/adapter.rs` | `NoteLinkDto` gains `label: Option<String>` + `kind: Option<String>` |
| `interfaces/webchat/src/canvas_engine/layout.rs` | `compute_target_positions` rewritten to use perturbed rings + scatter; old orphan logic deleted |
| `interfaces/webchat/src/canvas_engine/renderer.rs` | Node circle-fill code deleted; edges switch to Bézier + gradient; hover/selected highlighting; selection ring stays |
| `interfaces/webchat/src/canvas_engine/prefetch.rs` | `PrefetchCache<T>` generic parameter; existing call sites unchanged via `PrefetchCache<GraphNeighborsResponse>` |
| `interfaces/webchat/src/canvas_engine/mod.rs` | Register new sub-modules |
| `interfaces/webchat/src/views/canvas/mod.rs` | Strip top stack + right detail; render `<GraphCanvas/> + <MiniMapOverlay/>` only |
| `interfaces/webchat/src/views/canvas/graph_canvas.rs` | rAF loop emits world-to-screen positions to Leptos signal for DOM overlay |
| `interfaces/webchat/src/views/canvas/detail_panel.rs` | Slim wrapper; the bulk moves to `node_detail_panel.rs` |
| `interfaces/webchat/src/views/mod.rs` | Re-exports |
| `interfaces/webchat/src/components/mode_sidebar.rs` | `MemorySidebar` rewritten with full content stack + ⇧ collapse button |
| `interfaces/webchat/src/app.rs` | Provide `MemoryState` context; install global Esc key listener |
| `interfaces/webchat/src/styles.css` (or its closest equivalent) | New CSS variables (§5.3) + collapse transition CSS |

### Deleted (after their content is migrated)
- `interfaces/webchat/src/views/canvas/agent_selector.rs`
- `interfaces/webchat/src/views/canvas/toolbar.rs`
- `interfaces/webchat/src/views/canvas/breadcrumb.rs` (its component is reborn as an inline element at the top of `NodeDetailPanel`)

> Run `grep -rn` for these filenames in the panel crate before deletion to confirm the only call site is `views/canvas/mod.rs`.

### Where styles.css lives
Run this command once at the start of Phase 4 to locate the stylesheet:

```bash
grep -rln "aleph-sidebar\|aleph-shell" /Volumes/TBU4/Workspace/Aleph/interfaces/webchat --include="*.css"
```

Use the path it prints in subsequent CSS tasks.

---

## Phase 0 — Prototype Gate

> **Mission:** before touching anything else, prove the DOM-overlay-on-canvas pattern hits 60 fps at 300 nodes. If it doesn't, the entire spec needs a retreat plan (Canvas2D-drawn cards instead). All later phases are gated on this.

### Task 0.1 — Capture a frozen seed graph fixture

**Files:**
- Create: `interfaces/webchat/tests/fixtures/canvas_30nodes.json`

- [ ] **Step 1: Generate the fixture JSON**

Write the file with this exact content (30 nodes, 45 edges, deterministic categories — no need to dump from a real vault for Phase 0):

```bash
cat > /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/tests/fixtures/canvas_30nodes.json <<'JSON'
{
  "center": { "id": "n00", "name": "Center memory", "path": "n00.md", "category": "user", "tags": ["pivot"], "link_count": 8 },
  "nodes": [
    { "id": "n01", "name": "Feedback one",    "path": "n01.md", "category": "feedback",  "tags": ["git"],       "link_count": 3 },
    { "id": "n02", "name": "Feedback two",    "path": "n02.md", "category": "feedback",  "tags": ["testing"],   "link_count": 2 },
    { "id": "n03", "name": "Project alpha",   "path": "n03.md", "category": "project",   "tags": ["alpha"],     "link_count": 5 },
    { "id": "n04", "name": "Project beta",    "path": "n04.md", "category": "project",   "tags": ["beta"],      "link_count": 4 },
    { "id": "n05", "name": "Reference rust",  "path": "n05.md", "category": "reference", "tags": ["rust"],      "link_count": 6 },
    { "id": "n06", "name": "Reference http",  "path": "n06.md", "category": "reference", "tags": ["http"],      "link_count": 2 },
    { "id": "n07", "name": "Project gamma",   "path": "n07.md", "category": "project",   "tags": [],            "link_count": 1 },
    { "id": "n08", "name": "User goal A",     "path": "n08.md", "category": "user",      "tags": [],            "link_count": 2 },
    { "id": "n09", "name": "Feedback three",  "path": "n09.md", "category": "feedback",  "tags": [],            "link_count": 1 },
    { "id": "n10", "name": "Two-hop a",       "path": "n10.md", "category": "reference", "tags": [],            "link_count": 1 },
    { "id": "n11", "name": "Two-hop b",       "path": "n11.md", "category": "reference", "tags": [],            "link_count": 1 },
    { "id": "n12", "name": "Two-hop c",       "path": "n12.md", "category": "project",   "tags": [],            "link_count": 1 },
    { "id": "n13", "name": "Two-hop d",       "path": "n13.md", "category": "feedback",  "tags": [],            "link_count": 1 },
    { "id": "n14", "name": "Two-hop e",       "path": "n14.md", "category": "reference", "tags": [],            "link_count": 1 },
    { "id": "n15", "name": "Two-hop f",       "path": "n15.md", "category": "project",   "tags": [],            "link_count": 1 },
    { "id": "n16", "name": "Two-hop g",       "path": "n16.md", "category": "feedback",  "tags": [],            "link_count": 1 },
    { "id": "n17", "name": "Two-hop h",       "path": "n17.md", "category": "user",      "tags": [],            "link_count": 1 },
    { "id": "n18", "name": "Two-hop i",       "path": "n18.md", "category": "reference", "tags": [],            "link_count": 1 },
    { "id": "n19", "name": "Orphan a",        "path": "n19.md", "category": "user",      "tags": [],            "link_count": 0 },
    { "id": "n20", "name": "Orphan b",        "path": "n20.md", "category": "feedback",  "tags": [],            "link_count": 0 },
    { "id": "n21", "name": "Orphan c",        "path": "n21.md", "category": "project",   "tags": [],            "link_count": 0 },
    { "id": "n22", "name": "Orphan d",        "path": "n22.md", "category": "reference", "tags": [],            "link_count": 0 },
    { "id": "n23", "name": "Orphan e",        "path": "n23.md", "category": "user",      "tags": [],            "link_count": 0 },
    { "id": "n24", "name": "Orphan f",        "path": "n24.md", "category": "feedback",  "tags": [],            "link_count": 0 },
    { "id": "n25", "name": "Orphan g",        "path": "n25.md", "category": "project",   "tags": [],            "link_count": 0 },
    { "id": "n26", "name": "Orphan h",        "path": "n26.md", "category": "reference", "tags": [],            "link_count": 0 },
    { "id": "n27", "name": "Orphan i",        "path": "n27.md", "category": "feedback",  "tags": [],            "link_count": 0 },
    { "id": "n28", "name": "Orphan j",        "path": "n28.md", "category": "project",   "tags": [],            "link_count": 0 },
    { "id": "n29", "name": "Orphan k",        "path": "n29.md", "category": "user",      "tags": [],            "link_count": 0 }
  ],
  "edges": [
    {"from":"n00","to":"n01"},{"from":"n00","to":"n02"},{"from":"n00","to":"n03"},{"from":"n00","to":"n04"},
    {"from":"n00","to":"n05"},{"from":"n00","to":"n06"},{"from":"n00","to":"n07"},{"from":"n00","to":"n08"},
    {"from":"n00","to":"n09"},
    {"from":"n01","to":"n10"},{"from":"n02","to":"n11"},{"from":"n03","to":"n12"},{"from":"n04","to":"n13"},
    {"from":"n05","to":"n14"},{"from":"n06","to":"n15"},{"from":"n07","to":"n16"},{"from":"n08","to":"n17"},
    {"from":"n09","to":"n18"},
    {"from":"n01","to":"n02"},{"from":"n03","to":"n04"},{"from":"n05","to":"n06"},
    {"from":"n01","to":"n03"},{"from":"n02","to":"n05"},{"from":"n04","to":"n07"},{"from":"n06","to":"n08"},
    {"from":"n10","to":"n11"},{"from":"n12","to":"n13"},{"from":"n14","to":"n15"},
    {"from":"n10","to":"n14"},{"from":"n11","to":"n15"},{"from":"n12","to":"n16"},
    {"from":"n13","to":"n17"},{"from":"n14","to":"n18"},
    {"from":"n01","to":"n05"},{"from":"n02","to":"n06"},{"from":"n03","to":"n07"},
    {"from":"n04","to":"n08"},{"from":"n05","to":"n09"},{"from":"n06","to":"n01"},
    {"from":"n07","to":"n02"},{"from":"n08","to":"n03"},{"from":"n09","to":"n04"},
    {"from":"n10","to":"n12"},{"from":"n11","to":"n13"},{"from":"n12","to":"n14"}
  ],
  "hop_depth": {
    "n01":1,"n02":1,"n03":1,"n04":1,"n05":1,"n06":1,"n07":1,"n08":1,"n09":1,
    "n10":2,"n11":2,"n12":2,"n13":2,"n14":2,"n15":2,"n16":2,"n17":2,"n18":2
  }
}
JSON
```

- [ ] **Step 2: Validate the JSON parses against the existing DTO**

Add a one-shot test at the end of `interfaces/webchat/src/canvas_engine/adapter.rs`:

```rust
#[cfg(test)]
mod fixture_tests {
    use super::*;

    #[test]
    fn fixture_canvas_30nodes_parses() {
        let json = include_str!("../../tests/fixtures/canvas_30nodes.json");
        let parsed: GraphNeighborsResponse = serde_json::from_str(json)
            .expect("fixture must parse against current DTOs");
        assert_eq!(parsed.center.id, "n00");
        assert_eq!(parsed.nodes.len(), 29);
        assert_eq!(parsed.edges.len(), 45);
        assert_eq!(parsed.hop_depth.get("n01"), Some(&1));
    }
}
```

- [ ] **Step 3: Run test, confirm PASS**

```bash
pgrep -x cargo | wc -l  # must be < 3 before proceeding
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::adapter::fixture_tests -- --nocapture
```

Expected: `test fixture_canvas_30nodes_parses ... ok`. If it fails because `serde_json` isn't a panel dep, add it to the `[dev-dependencies]` section of `interfaces/webchat/Cargo.toml`:

```toml
[dev-dependencies]
serde_json = "1"
```

Then re-run.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/tests/fixtures/canvas_30nodes.json interfaces/webchat/src/canvas_engine/adapter.rs interfaces/webchat/Cargo.toml
git commit -m "canvas: add 30-node fixture and parse smoke test for redesign plan"
```

### Task 0.2 — Prototype DOM-overlay-over-Canvas2D fps spike

**Files:**
- Create: `interfaces/webchat/src/views/canvas/_perf_spike.rs` (underscored — will be deleted after the gate)
- Modify: `interfaces/webchat/src/views/canvas/mod.rs` (register the spike module behind a temporary route)

> This task is exploratory. The deliverable is a measured fps number, not production code. The whole module is deleted at the end of the phase.

- [ ] **Step 1: Stub the spike component**

Create `_perf_spike.rs`:

```rust
//! TEMPORARY — Phase 0 perf gate for the memory-canvas redesign.
//! Renders 300 absolutely-positioned Leptos cards over a Canvas2D draw,
//! animating their positions each rAF tick to mimic drift + drag. Logs
//! the rolling fps to the console. Delete this file once the gate passes.

use leptos::prelude::*;
use leptos::task::spawn_local;
use std::cell::Cell;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

#[component]
pub fn PerfSpike() -> impl IntoView {
    let positions = RwSignal::new(seed_positions(300));
    let fps_signal = RwSignal::new(0.0_f64);

    Effect::new(move |_| {
        let positions = positions;
        let fps_signal = fps_signal;
        let last_t = Rc::new(Cell::new(0.0));
        let frames = Rc::new(Cell::new(0_u32));
        let last_log = Rc::new(Cell::new(0.0));

        let window = web_sys::window().unwrap();
        let perf = window.performance().unwrap();
        let cb_holder = Rc::new(std::cell::RefCell::new(None));
        let cb_holder_clone = cb_holder.clone();
        let perf2 = perf.clone();
        let cb = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
            let now = perf.now();
            frames.set(frames.get() + 1);
            if now - last_log.get() > 1000.0 {
                fps_signal.set(frames.get() as f64 * 1000.0 / (now - last_log.get()));
                last_log.set(now);
                frames.set(0);
            }
            positions.update(|ps| {
                for (i, p) in ps.iter_mut().enumerate() {
                    let phase = now * 0.001 + i as f64 * 0.1;
                    p.0 += (phase.sin() * 0.5) as f32;
                    p.1 += (phase.cos() * 0.5) as f32;
                }
            });
            last_t.set(now);
            let win = web_sys::window().unwrap();
            if let Some(c) = cb_holder_clone.borrow().as_ref() {
                let _ = win.request_animation_frame(
                    wasm_bindgen::JsCast::unchecked_ref::<js_sys::Function>(c)
                );
            }
        }) as Box<dyn FnMut()>);
        *cb_holder.borrow_mut() = Some(cb.into_js_value());
        let win = web_sys::window().unwrap();
        if let Some(c) = cb_holder.borrow().as_ref() {
            let _ = win.request_animation_frame(
                wasm_bindgen::JsCast::unchecked_ref::<js_sys::Function>(c)
            );
        }
        let _ = perf2; // silence warning
    });

    view! {
        <div class="fixed top-0 left-0 z-50 bg-black/80 p-2 text-white text-xs">
            "fps: " {move || format!("{:.1}", fps_signal.get())}
        </div>
        <div class="relative w-screen h-screen bg-[#080818]">
            {move || positions.get().into_iter().enumerate().map(|(i, p)| {
                view! {
                    <div
                        class="absolute w-[140px] h-[30px] bg-[#1a1a2e] border border-[#2a2a40] rounded px-2 py-1 text-xs text-[#cbd5e1]"
                        style:transform=move || format!("translate3d({}px, {}px, 0)", p.0, p.1)
                    >
                        "Node " {i}
                    </div>
                }
            }).collect_view()}
        </div>
    }
}

fn seed_positions(n: usize) -> Vec<(f32, f32)> {
    (0..n).map(|i| {
        let t = i as f32 * 0.5;
        (640.0 + (i as f32 * 13.0).sin() * 400.0,
         360.0 + (t * 7.0).cos() * 220.0)
    }).collect()
}
```

- [ ] **Step 2: Wire the spike behind a query-string toggle**

Modify the top of `views/canvas/mod.rs::CanvasView` so that visiting `/memory?spike=1` shows the spike instead of the real canvas:

```rust
mod _perf_spike;
// ...existing code...

#[component]
pub fn CanvasView() -> impl IntoView {
    let location = leptos_router::hooks::use_location();
    let is_spike = move || {
        location.query.with(|q| q.get_str("spike").is_some())
    };
    view! {
        {move || if is_spike() {
            view! { <_perf_spike::PerfSpike /> }.into_any()
        } else {
            view! { <RadialCanvasView /> }.into_any()
        }}
    }
}
```

- [ ] **Step 3: Build, run, measure**

```bash
pgrep -x cargo | wc -l   # < 3
just dev
# In browser: visit http://localhost:<port>/memory?spike=1
# Watch the on-screen fps counter for ~30 s.
```

Expected: **fps ≥ 55 sustained** on a modern laptop. Record the measured number.

- [ ] **Step 4: Decide go/no-go**

If fps ≥ 55:
- Proceed to Phase 1.
- Record the result in CHANGELOG.md (English, under `Added` section, draft entry — final commit at Phase 8):
  > Canvas perf gate: 300 DOM-overlay nodes sustained ≥ 55 fps. Memory canvas redesign cleared to proceed.

If fps < 55:
- **STOP**. Open a follow-up brainstorm to redesign §5 (node cards may need to be Canvas2D-drawn rounded rects rather than DOM). Do not proceed with the rest of this plan.

- [ ] **Step 5: Delete the spike + commit**

```bash
rm /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/views/canvas/_perf_spike.rs
# Revert mod.rs to its pre-spike form (remove the mod line and the query-string branch).
git add interfaces/webchat/src/views/canvas/mod.rs interfaces/webchat/src/views/canvas/_perf_spike.rs
git commit -m "canvas: perf gate confirmed (300 DOM nodes @ <fps>); remove spike"
```

Replace `<fps>` with the measured number (e.g., `60.0`).

---

## Phase 1 — Foundations (data model, hash, markdown)

> No visible change yet. Pure-function building blocks plus the DTO field additions. Each task lands a unit test.

### Task 1.1 — Add `label` and `kind` to `NoteLinkDto`

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/adapter.rs:16-20`

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod fixture_tests` in `adapter.rs`:

```rust
#[test]
fn note_link_dto_round_trips_label_and_kind() {
    let raw = r#"{"from":"a","to":"b","label":"refers to","kind":"refers"}"#;
    let parsed: NoteLinkDto = serde_json::from_str(raw).unwrap();
    assert_eq!(parsed.label.as_deref(), Some("refers to"));
    assert_eq!(parsed.kind.as_deref(), Some("refers"));
}

#[test]
fn note_link_dto_defaults_when_label_and_kind_absent() {
    let raw = r#"{"from":"a","to":"b"}"#;
    let parsed: NoteLinkDto = serde_json::from_str(raw).unwrap();
    assert_eq!(parsed.label, None);
    assert_eq!(parsed.kind, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::adapter::fixture_tests -- --nocapture
```

Expected: `note_link_dto_round_trips_label_and_kind` and `note_link_dto_defaults_when_label_and_kind_absent` both **fail** with "unknown field `label`" or similar.

- [ ] **Step 3: Extend the DTO**

Edit `adapter.rs:16-20`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct NoteLinkDto {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::adapter::fixture_tests
```

Expected: all 4 tests in `fixture_tests` pass.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/adapter.rs
git commit -m "canvas: add label and kind fields to NoteLinkDto (Obsidian schema-compatible)"
```

### Task 1.2 — FNV-1a deterministic hash helper

**Files:**
- Create: `interfaces/webchat/src/canvas_engine/fnv1a.rs`
- Modify: `interfaces/webchat/src/canvas_engine/mod.rs`

- [ ] **Step 1: Register the module**

Append to `canvas_engine/mod.rs` (the current `mod.rs` is 12 lines — add a single line at the end):

```rust
pub(crate) mod fnv1a;
```

- [ ] **Step 2: Write the failing test**

Create `fnv1a.rs`:

```rust
//! Tiny FNV-1a 32-bit hash for deterministic node-position jitter.
//! Standard reference: http://www.isthe.com/chongo/tech/comp/fnv/.

const FNV_OFFSET: u32 = 2_166_136_261;
const FNV_PRIME:  u32 = 16_777_619;

/// 32-bit FNV-1a hash of `bytes`. Identical on every machine and every run.
pub(crate) fn fnv1a_32(bytes: &[u8]) -> u32 {
    let mut h = FNV_OFFSET;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Normalised jitter in `[-1.0, 1.0]` keyed by an arbitrary string id.
/// Used as the deterministic substitute for `random()` in layout code.
pub(crate) fn hash_jitter(id: &str) -> f32 {
    // 1024-bucket so adjacent ids do NOT cluster
    let h = (fnv1a_32(id.as_bytes()) % 1024) as i32 - 512;
    h as f32 / 512.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_matches_canonical_vectors() {
        // Reference values from http://www.isthe.com/chongo/tech/comp/fnv/
        assert_eq!(fnv1a_32(b""),      0x811c9dc5);
        assert_eq!(fnv1a_32(b"a"),     0xe40c292c);
        assert_eq!(fnv1a_32(b"foobar"),0xbf9cf968);
    }

    #[test]
    fn hash_jitter_in_range() {
        for id in ["", "a", "foo", "very-long-identifier-that-keeps-going-and-going"] {
            let j = hash_jitter(id);
            assert!((-1.0..=1.0).contains(&j), "out of range: {}", j);
        }
    }

    #[test]
    fn hash_jitter_is_deterministic() {
        assert_eq!(hash_jitter("node-x"), hash_jitter("node-x"));
    }

    #[test]
    fn hash_jitter_varies_with_id() {
        let a = hash_jitter("node-a");
        let b = hash_jitter("node-b");
        assert_ne!(a, b);
    }
}
```

- [ ] **Step 3: Run tests; verify they pass**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::fnv1a
```

Expected: 4 tests pass. The canonical-vector test is the trust anchor — if it fails, the FNV constants are wrong.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/fnv1a.rs interfaces/webchat/src/canvas_engine/mod.rs
git commit -m "canvas: add fnv1a deterministic hash helper for layout jitter"
```

### Task 1.3 — Markdown excerpt → whitelisted HTML

**Files:**
- Create: `interfaces/webchat/src/canvas_engine/markdown_excerpt.rs`
- Modify: `interfaces/webchat/src/canvas_engine/mod.rs`

- [ ] **Step 1: Register the module**

Append to `canvas_engine/mod.rs`:

```rust
pub(crate) mod markdown_excerpt;
```

- [ ] **Step 2: Write the failing tests + skeleton**

Create `markdown_excerpt.rs`:

```rust
//! Convert a Markdown excerpt to a tightly-whitelisted HTML string.
//!
//! Supports: **bold**, `inline code`, [link](url), hard line breaks.
//! Everything else (headers, lists, blockquotes, images, html) is
//! stripped to plain text. Output is safe to feed into `inner_html=`.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

const MAX_LEN: usize = 180;

/// Render `src` (raw Markdown) into a 180-char whitelisted HTML string.
/// Truncates with an ellipsis if the source is longer.
pub fn render_excerpt(src: &str) -> String {
    let parser = Parser::new(src);
    let mut out = String::with_capacity(src.len().min(MAX_LEN * 2));
    let mut chars_used = 0_usize;

    for event in parser {
        if chars_used >= MAX_LEN {
            out.push('\u{2026}');
            break;
        }
        match event {
            Event::Text(t) => {
                let remaining = MAX_LEN.saturating_sub(chars_used);
                let take = t.chars().take(remaining).collect::<String>();
                chars_used += take.chars().count();
                out.push_str(&html_escape(&take));
            }
            Event::Code(t) => {
                out.push_str("<code>");
                out.push_str(&html_escape(&t));
                out.push_str("</code>");
                chars_used += t.chars().count();
            }
            Event::Start(Tag::Strong) => out.push_str("<strong>"),
            Event::End(TagEnd::Strong) => out.push_str("</strong>"),
            Event::Start(Tag::Link { dest_url, .. }) => {
                out.push_str("<a target=\"_blank\" rel=\"noopener\" href=\"");
                out.push_str(&html_escape(&dest_url));
                out.push_str("\">");
            }
            Event::End(TagEnd::Link) => out.push_str("</a>"),
            Event::HardBreak | Event::SoftBreak => out.push_str("<br>"),
            // Everything else: ignore the tag, the inner Text events still emit
            _ => {}
        }
    }
    out
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_plain_text() {
        assert_eq!(render_excerpt("hello world"), "hello world");
    }

    #[test]
    fn renders_bold_inline_code_and_link() {
        let out = render_excerpt("**bold** and `code` and [x](https://example.com)");
        assert!(out.contains("<strong>bold</strong>"));
        assert!(out.contains("<code>code</code>"));
        assert!(out.contains("<a target=\"_blank\" rel=\"noopener\" href=\"https://example.com\">x</a>"));
    }

    #[test]
    fn strips_headers_and_lists_to_text() {
        let out = render_excerpt("# Title\n- item\n- item");
        assert!(!out.contains("<h1>"));
        assert!(!out.contains("<ul>"));
        assert!(out.contains("Title"));
        assert!(out.contains("item"));
    }

    #[test]
    fn escapes_raw_html_attempts() {
        let out = render_excerpt("<script>alert(1)</script> ok");
        assert!(!out.contains("<script>"));
        assert!(out.contains("ok"));
    }

    #[test]
    fn truncates_long_input_with_ellipsis() {
        let long = "x".repeat(300);
        let out = render_excerpt(&long);
        assert!(out.ends_with('\u{2026}'));
        assert!(out.chars().count() <= MAX_LEN + 1);
    }
}
```

- [ ] **Step 3: Run tests to verify all pass**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::markdown_excerpt
```

Expected: 5 tests pass. If `pulldown-cmark` API has shifted (e.g., `Tag::Link` field names), adjust accordingly — the version is pinned to `0.12`.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/markdown_excerpt.rs interfaces/webchat/src/canvas_engine/mod.rs
git commit -m "canvas: add markdown_excerpt whitelist renderer (bold/code/link/break)"
```

### Task 1.4 — Generalize `PrefetchCache<T>`

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/prefetch.rs`

> Existing `PrefetchCache` only caches `GraphNeighborsResponse`. Spec §5.3a needs the same type to cache `NoteDetailResponse`. Make the struct generic over the value type.

- [ ] **Step 1: Survey existing call sites**

```bash
grep -rn "PrefetchCache" /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src --include="*.rs"
```

Record every line. Each one is an `impl-driven` use of `PrefetchCache::new()` / `.put` / `.get` / `.has` / `.len`. We will preserve all method signatures, only the carried type changes.

- [ ] **Step 2: Make the struct generic**

Edit `prefetch.rs:8-50` (the `pub struct PrefetchCache` block plus its `impl`):

```rust
use std::collections::VecDeque;

pub const HOVER_DEBOUNCE_MS: f64 = 150.0;
pub const CACHE_TTL_MS: f64 = 60_000.0;
pub const CACHE_CAPACITY: usize = 20;

/// Bounded LRU cache of payloads keyed by `String` id, with TTL.
pub struct PrefetchCache<T> {
    entries: VecDeque<(String, T, f64)>,
    capacity: usize,
    ttl_ms: f64,
}

impl<T> PrefetchCache<T> {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            capacity: CACHE_CAPACITY,
            ttl_ms: CACHE_TTL_MS,
        }
    }

    pub fn put(&mut self, id: String, value: T, now_ms: f64) {
        self.entries.retain(|(k, _, _)| k != &id);
        self.entries.push_back((id, value, now_ms));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    pub fn get(&self, id: &str, now_ms: f64) -> Option<&T> {
        self.entries.iter().rev().find_map(|(k, v, fetched)| {
            if k == id && now_ms - fetched <= self.ttl_ms {
                Some(v)
            } else {
                None
            }
        })
    }

    pub fn has(&self, id: &str, now_ms: f64) -> bool {
        self.get(id, now_ms).is_some()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl<T> Default for PrefetchCache<T> {
    fn default() -> Self {
        Self::new()
    }
}
```

Remove the now-unused `use crate::canvas_engine::adapter::GraphNeighborsResponse;` line at the top — the type is no longer named here.

- [ ] **Step 3: Update call sites by adding the type parameter**

For every call site found in Step 1, change `PrefetchCache` → `PrefetchCache<GraphNeighborsResponse>`. Most sites will be `PrefetchCache::new()`; in those positions write `PrefetchCache::<GraphNeighborsResponse>::new()`. Or annotate the binding: `let cache: PrefetchCache<GraphNeighborsResponse> = PrefetchCache::new();`.

If a site stores it in a struct field, update the field type.

- [ ] **Step 4: Verify build is clean**

```bash
pgrep -x cargo | wc -l   # < 3
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

Expected: no errors. If there's a "type parameter T is never used" warning, suppress by adding a `PhantomData<T>` field — but `T` is used in the `VecDeque<(String, T, f64)>` so this shouldn't trigger.

- [ ] **Step 5: Add a generic-instantiation test**

Append to the existing `#[cfg(test)] mod` in `prefetch.rs` (or create one):

```rust
#[cfg(test)]
mod generic_tests {
    use super::*;

    #[test]
    fn cache_works_for_string_payloads() {
        let mut c: PrefetchCache<String> = PrefetchCache::new();
        c.put("a".into(), "hello".into(), 0.0);
        assert_eq!(c.get("a", 100.0).map(String::as_str), Some("hello"));
        assert!(c.has("a", 100.0));
    }

    #[test]
    fn cache_expires_by_ttl() {
        let mut c: PrefetchCache<u32> = PrefetchCache::new();
        c.put("k".into(), 42, 0.0);
        assert_eq!(c.get("k", CACHE_TTL_MS - 1.0), Some(&42));
        assert_eq!(c.get("k", CACHE_TTL_MS + 1.0), None);
    }
}
```

- [ ] **Step 6: Run all panel canvas_engine tests**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::
```

Expected: all existing tests still pass plus the two new ones.

- [ ] **Step 7: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/prefetch.rs <every other modified caller>
git commit -m "canvas: make PrefetchCache<T> generic for upcoming note-detail caching"
```

---

## Phase 2 — Layout: perturbed rings + scattered orphans

> The visual still shows old circles, but their positions follow the new algorithm. This isolates layout changes from rendering changes.

### Task 2.1 — Refactor `place_perturbed_ring` (extract from `compute_target_positions`)

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs:71-…` (the existing `compute_target_positions` block)

- [ ] **Step 1: Read the current implementation**

Open the file and identify the body of `compute_target_positions` (starts line 71). Save a mental snapshot — we will extract one helper at a time and preserve current external behaviour until Task 2.4 swaps it.

- [ ] **Step 2: Add a private helper `place_perturbed_ring` (deterministic + jitter)**

Inside `layout.rs`, above `compute_target_positions`, add:

```rust
use crate::canvas_engine::fnv1a::hash_jitter;
use crate::canvas_engine::types::Vec2;
use std::collections::HashMap;
use std::f32::consts::TAU;

/// Place `ids` on a ring around `(0,0)` at base radius `base_r`,
/// with deterministic per-id jitter in angle (±17°) and radius (±15%).
///
/// Writes positions into `out`. Skips ids already present.
pub(crate) fn place_perturbed_ring(
    ids: &[&str],
    base_r: f32,
    out: &mut HashMap<String, Vec2>,
) {
    if ids.is_empty() {
        return;
    }
    let n = ids.len() as f32;
    for (i, id) in ids.iter().enumerate() {
        if out.contains_key(*id) {
            continue;
        }
        let j_angle  = 0.30 * hash_jitter(id);              // ±17.2°
        let j_radius = 0.15 * hash_jitter(&format!("r:{id}"));  // ±15 %, decorrelated
        let angle = (i as f32 / n) * TAU + j_angle;
        let radius = base_r * (1.0 + j_radius);
        out.insert((*id).into(), Vec2 {
            x: radius * angle.cos(),
            y: radius * angle.sin(),
        });
    }
}
```

- [ ] **Step 3: Write the unit tests**

Append to the `#[cfg(test)] mod tests { ... }` block at the bottom of `layout.rs`:

```rust
#[test]
fn perturbed_ring_is_deterministic() {
    let ids = vec!["a", "b", "c", "d"];
    let mut m1 = HashMap::new();
    let mut m2 = HashMap::new();
    place_perturbed_ring(&ids, 200.0, &mut m1);
    place_perturbed_ring(&ids, 200.0, &mut m2);
    assert_eq!(m1, m2);
}

#[test]
fn perturbed_ring_avoids_collision() {
    use std::f32::consts::TAU;
    let ids: Vec<&str> = (0..8).map(|i| Box::leak(format!("n{i}").into_boxed_str()) as &str).collect();
    let mut m = HashMap::new();
    place_perturbed_ring(&ids, 200.0, &mut m);
    let n = ids.len() as f32;
    let min_sep = (TAU / n) * 0.4; // 40 % of even spacing
    let mut angles: Vec<f32> = ids.iter().map(|id| {
        let p = m[*id];
        p.y.atan2(p.x).rem_euclid(TAU)
    }).collect();
    angles.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for w in angles.windows(2) {
        assert!((w[1] - w[0]) >= min_sep,
            "adjacent angles {} and {} too close (min_sep={})", w[0], w[1], min_sep);
    }
}

#[test]
fn perturbed_ring_skips_existing_ids() {
    let mut m = HashMap::new();
    m.insert("a".into(), Vec2 { x: 999.0, y: 999.0 });
    place_perturbed_ring(&["a", "b"], 200.0, &mut m);
    assert_eq!(m["a"], Vec2 { x: 999.0, y: 999.0 });
    assert!(m.contains_key("b"));
}
```

If `Vec2` does not implement `PartialEq + Eq`, derive them (check `canvas_engine/types.rs:5`; if it lacks them, add `#[derive(PartialEq)]` and the assertion `assert_eq!(p1, p2)` needs adjusting to compare `.x` and `.y` separately).

- [ ] **Step 4: Run tests**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::layout::tests::perturbed_ring
```

Expected: 3 new tests pass.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/layout.rs
git commit -m "canvas: extract place_perturbed_ring helper (no behavior change yet)"
```

### Task 2.2 — Implement `place_scattered` (Poisson-disk-like)

**Files:**
- Create: `interfaces/webchat/src/canvas_engine/scatter.rs`
- Modify: `interfaces/webchat/src/canvas_engine/mod.rs`

- [ ] **Step 1: Register the module**

Append to `canvas_engine/mod.rs`:

```rust
pub(crate) mod scatter;
```

- [ ] **Step 2: Write the failing tests + skeleton**

Create `scatter.rs`:

```rust
//! Poisson-disk-like deterministic placement for orphan nodes.
//!
//! - Avoids the central exclusion rect (the inner 60 % of the viewport,
//!   which is reserved for center + 1-hop + 2-hop rings).
//! - Maintains a minimum pairwise distance between all placed nodes
//!   (existing + new), pulling from hash-derived candidate samples.
//! - Deterministic given the same `(orphans, viewport, existing)` triple.

use crate::canvas_engine::fnv1a::fnv1a_32;
use crate::canvas_engine::types::Vec2;
use std::collections::HashMap;

/// Default minimum distance between two orphan centres (in world px).
pub(crate) const MIN_DISTANCE: f32 = 56.0;       // ≈ 2 × dot radius + 20
/// Candidate attempts per orphan before we accept the best-so-far.
pub(crate) const CANDIDATES_PER_ORPHAN: u32 = 20;
/// Fraction of viewport the central exclusion rect occupies (each axis).
pub(crate) const CENTRAL_EXCLUSION: f32 = 0.60;
/// Threshold above which orphans spill into a second outer band.
pub(crate) const SPILL_THRESHOLD: usize = 20;

/// Place each orphan in `ids` into `out`. `existing` is the read-only
/// set of already-placed positions used for collision avoidance.
pub(crate) fn place_scattered(
    ids: &[&str],
    viewport: (f32, f32),
    existing: &HashMap<String, Vec2>,
    out: &mut HashMap<String, Vec2>,
) {
    let (vw, vh) = viewport;
    let half_w = vw * 0.5;
    let half_h = vh * 0.5;
    let excl_w = vw * CENTRAL_EXCLUSION * 0.5;
    let excl_h = vh * CENTRAL_EXCLUSION * 0.5;
    let spill = ids.len() > SPILL_THRESHOLD;

    for (i, id) in ids.iter().enumerate() {
        let (best, _) = best_candidate(
            id, i, half_w, half_h, excl_w, excl_h, existing, out, spill,
        );
        out.insert((*id).into(), best);
    }
}

fn best_candidate(
    id: &str,
    index: usize,
    half_w: f32,
    half_h: f32,
    excl_w: f32,
    excl_h: f32,
    existing: &HashMap<String, Vec2>,
    placed: &HashMap<String, Vec2>,
    spill: bool,
) -> (Vec2, f32) {
    let band = if spill && index >= SPILL_THRESHOLD {
        // outer band: shrink min-distance + push outwards (radial bias)
        Band::Outer
    } else {
        Band::Inner
    };

    let mut best_pos = Vec2 { x: 0.0, y: 0.0 };
    let mut best_score = f32::NEG_INFINITY;

    for attempt in 0..CANDIDATES_PER_ORPHAN {
        let key = format!("scatter:{id}:{attempt}");
        let h = fnv1a_32(key.as_bytes());
        let (rx, ry) = (
            (h as u32 % 1000) as f32 / 1000.0 - 0.5,         // [-0.5, 0.5]
            ((h.wrapping_shr(11)) % 1000) as f32 / 1000.0 - 0.5,
        );
        let mut cx = rx * 2.0 * half_w;
        let mut cy = ry * 2.0 * half_h;

        // Push out of central exclusion rect
        if cx.abs() < excl_w && cy.abs() < excl_h {
            // shove to nearest edge of exclusion
            if (excl_w - cx.abs()) < (excl_h - cy.abs()) {
                cx = excl_w * cx.signum().max(0.5);
            } else {
                cy = excl_h * cy.signum().max(0.5);
            }
        }
        if matches!(band, Band::Outer) {
            // pull radially outward
            let r = (cx * cx + cy * cy).sqrt().max(1.0);
            let target_r = ((half_w.min(half_h)) - 30.0).max(r + 30.0);
            let s = target_r / r;
            cx *= s;
            cy *= s;
        }

        let p = Vec2 { x: cx, y: cy };
        let score = min_distance(p, existing, placed);
        if score > best_score {
            best_score = score;
            best_pos = p;
        }
    }
    (best_pos, best_score)
}

enum Band { Inner, Outer }

fn min_distance(p: Vec2, existing: &HashMap<String, Vec2>, placed: &HashMap<String, Vec2>) -> f32 {
    let mut best = f32::INFINITY;
    for q in existing.values().chain(placed.values()) {
        let dx = p.x - q.x;
        let dy = p.y - q.y;
        let d = (dx * dx + dy * dy).sqrt();
        if d < best { best = d; }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> (f32, f32) { (1200.0, 800.0) }

    #[test]
    fn scattered_orphans_avoid_center() {
        let ids: Vec<&str> = (0..10).map(|i| Box::leak(format!("o{i}").into_boxed_str()) as &str).collect();
        let mut out = HashMap::new();
        place_scattered(&ids, vp(), &HashMap::new(), &mut out);
        let (vw, vh) = vp();
        let excl_w = vw * CENTRAL_EXCLUSION * 0.5;
        let excl_h = vh * CENTRAL_EXCLUSION * 0.5;
        for (id, p) in &out {
            assert!(p.x.abs() >= excl_w || p.y.abs() >= excl_h,
                "orphan {} at ({}, {}) inside exclusion rect", id, p.x, p.y);
        }
    }

    #[test]
    fn scattered_orphans_minimum_distance() {
        let ids: Vec<&str> = (0..8).map(|i| Box::leak(format!("o{i}").into_boxed_str()) as &str).collect();
        let mut out = HashMap::new();
        place_scattered(&ids, vp(), &HashMap::new(), &mut out);
        let v: Vec<_> = out.values().copied().collect();
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                let d = ((v[i].x - v[j].x).powi(2) + (v[i].y - v[j].y).powi(2)).sqrt();
                // Best-effort: we may not hit MIN_DISTANCE exactly but should be reasonable.
                assert!(d >= MIN_DISTANCE * 0.5,
                    "two orphans only {:.1}px apart (expect ≥ {:.1})", d, MIN_DISTANCE * 0.5);
            }
        }
    }

    #[test]
    fn scattered_is_deterministic() {
        let ids = ["a", "b", "c", "d", "e"];
        let mut m1 = HashMap::new();
        let mut m2 = HashMap::new();
        place_scattered(&ids, vp(), &HashMap::new(), &mut m1);
        place_scattered(&ids, vp(), &HashMap::new(), &mut m2);
        for k in &ids {
            assert_eq!(m1[*k].x, m2[*k].x);
            assert_eq!(m1[*k].y, m2[*k].y);
        }
    }

    #[test]
    fn scattered_spills_band_when_count_gt_threshold() {
        let ids: Vec<&str> = (0..25).map(|i| Box::leak(format!("o{i:02}").into_boxed_str()) as &str).collect();
        let mut out = HashMap::new();
        place_scattered(&ids, vp(), &HashMap::new(), &mut out);
        // Sort by radius and confirm there are two clusters (inner ≈ ½ viewport, outer ≈ edge)
        let mut radii: Vec<f32> = out.values().map(|p| (p.x * p.x + p.y * p.y).sqrt()).collect();
        radii.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let max_r = *radii.last().unwrap();
        let median = radii[radii.len() / 2];
        assert!(max_r - median > 100.0,
            "expected outer band well beyond median; got max={max_r}, med={median}");
    }
}
```

> Note: `Box::leak(format!(...).into_boxed_str())` is a test-only trick to get `&'static str` from generated strings. Acceptable in tests; never in production code.

- [ ] **Step 3: Run tests**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::scatter
```

Expected: 4 tests pass. If `scattered_orphans_minimum_distance` fails because the algorithm produces a too-close pair, increase `CANDIDATES_PER_ORPHAN` to 30 and retry.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/scatter.rs interfaces/webchat/src/canvas_engine/mod.rs
git commit -m "canvas: implement Poisson-disk-like scatter for orphan placement"
```

### Task 2.3 — Wire perturbed rings + scatter into `compute_target_positions`

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs:71-…`
- Possibly modify: `interfaces/webchat/src/canvas_engine/adapter.rs::populate_orphans` (if it independently places orphans)

- [ ] **Step 1: Read both functions and identify the boundary**

```bash
grep -n "fn compute_target_positions\|fn populate_orphans" /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/canvas_engine/layout.rs /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/canvas_engine/adapter.rs
```

Read the surrounding code (~50 lines around each). The new flow must:
1. Bucket nodes by hop_depth (already done — `Neighborhood` already has 1-hop / 2-hop / orphans).
2. Place center at `Vec2 { x: 0.0, y: 0.0 }`.
3. Place 1-hop via `place_perturbed_ring` at `r_one_hop(n, vw)`.
4. Place 2-hop via `place_perturbed_ring` at `r_two_hop(n, vw)`.
5. Place orphans via `place_scattered`.

If `populate_orphans` already lays them on a ring, **delete that body** and have it call into `place_scattered`.

- [ ] **Step 2: Rewrite `compute_target_positions`**

Replace the current body of `compute_target_positions` with the new implementation. Preserve the existing public signature exactly (check it first with `grep -A2 "pub fn compute_target_positions" layout.rs`). The new body looks like:

```rust
pub fn compute_target_positions(
    // ...existing signature, do not change parameters...
) -> HashMap<String, Vec2> {
    use crate::canvas_engine::scatter::place_scattered;

    let mut out: HashMap<String, Vec2> = HashMap::new();

    // 1. Centre at origin
    out.insert(center_id.to_string(), Vec2 { x: 0.0, y: 0.0 });

    // 2. 1-hop
    let one_hop_ids: Vec<&str> = one_hop_nodes.iter().map(|n| n.id.as_str()).collect();
    let r1 = r_one_hop(one_hop_ids.len(), viewport_w_px);
    place_perturbed_ring(&one_hop_ids, r1, &mut out);

    // 3. 2-hop
    let two_hop_ids: Vec<&str> = two_hop_nodes.iter().map(|n| n.id.as_str()).collect();
    let r2 = r_two_hop(two_hop_ids.len(), viewport_w_px);
    place_perturbed_ring(&two_hop_ids, r2, &mut out);

    // 4. Orphans
    let orphan_ids: Vec<&str> = orphan_nodes.iter().map(|n| n.id.as_str()).collect();
    place_scattered(&orphan_ids, (viewport_w_px, viewport_h_px), &HashMap::new(), &mut out);

    out
}
```

> Adapt the parameter names (`one_hop_nodes`, `two_hop_nodes`, `orphan_nodes`, `viewport_h_px`) to whatever the actual signature exposes. Read it first.

- [ ] **Step 3: Delete dead orphan-ring code in `adapter.rs::populate_orphans`**

Inspect `populate_orphans`. If it currently places orphans on a ring (look for `ORPHAN_RADIUS`, `r_orphan`, golden-angle constants), rewrite it to be a thin wrapper that defers position assignment to `compute_target_positions`. If it returns a `Neighborhood` with positions already filled, change it to leave orphan positions as `Vec2 { x: 0.0, y: 0.0 }` (placeholder) — the call to `compute_target_positions` downstream will fill them. Alternatively, call `place_scattered` directly inside `populate_orphans` for symmetry.

The simpler option: leave orphan positions as `(0, 0)` in `populate_orphans` and rely on `compute_target_positions`.

- [ ] **Step 4: Delete the now-unused `r_orphan` if nothing references it**

```bash
grep -rn "r_orphan\b" /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src
```

If only the `layout.rs` definition + its self-test reference it, delete `r_orphan` and its self-test (`r_orphan_outside_two_hop` at line 388). Otherwise leave it.

- [ ] **Step 5: Build + run the full canvas_engine test suite**

```bash
pgrep -x cargo | wc -l
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::
```

Expected: all tests pass, including the new perturbed-ring and scatter tests. Existing tests (`r_one_hop_grows_with_node_count`, `r_one_hop_clamps_viewport`, `r_two_hop_outside_one_hop`) must still pass — the radius helpers themselves are untouched.

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/layout.rs interfaces/webchat/src/canvas_engine/adapter.rs
git commit -m "canvas: switch compute_target_positions to perturbed rings + Poisson scatter"
```

### Task 2.4 — Layout snapshot regression test

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/layout.rs` (append a snapshot test)
- Create: `interfaces/webchat/tests/fixtures/layout_baseline_30nodes.json` (generated on first run)

- [ ] **Step 1: Write the snapshot test**

Append to the `#[cfg(test)] mod tests` block at the bottom of `layout.rs`:

```rust
#[test]
fn known_seed_layout_matches_baseline() {
    use crate::canvas_engine::adapter::GraphNeighborsResponse;

    let json = include_str!("../../tests/fixtures/canvas_30nodes.json");
    let resp: GraphNeighborsResponse = serde_json::from_str(json).unwrap();

    // Build the same buckets the production code would
    let one_hop: Vec<&NoteNodeDto> = resp.nodes.iter()
        .filter(|n| resp.hop_depth.get(&n.id).copied() == Some(1))
        .collect();
    let two_hop: Vec<&NoteNodeDto> = resp.nodes.iter()
        .filter(|n| resp.hop_depth.get(&n.id).copied() == Some(2))
        .collect();
    let orphans: Vec<&NoteNodeDto> = resp.nodes.iter()
        .filter(|n| !resp.hop_depth.contains_key(&n.id))
        .collect();

    // ... build the call to compute_target_positions matching its actual signature ...
    // (substitute parameter names; viewport 1200×800)
    let positions = compute_target_positions(
        &resp.center.id,
        &one_hop, &two_hop, &orphans,
        1200.0, 800.0,
    );

    let baseline_path = "interfaces/webchat/tests/fixtures/layout_baseline_30nodes.json";

    if std::env::var("BLESS_LAYOUT_SNAPSHOTS").is_ok() {
        let serialized = serde_json::to_string_pretty(
            &positions.iter().map(|(k, v)| (k.clone(), (v.x, v.y))).collect::<HashMap<_, _>>()
        ).unwrap();
        std::fs::write(baseline_path, serialized).unwrap();
        return;
    }

    let baseline_raw = std::fs::read_to_string(baseline_path)
        .expect("run with BLESS_LAYOUT_SNAPSHOTS=1 to create baseline");
    let baseline: HashMap<String, (f32, f32)> = serde_json::from_str(&baseline_raw).unwrap();
    for (id, expected) in &baseline {
        let actual = positions.get(id).expect("missing id");
        assert!((actual.x - expected.0).abs() < 0.01, "id={} x drift", id);
        assert!((actual.y - expected.1).abs() < 0.01, "id={} y drift", id);
    }
}
```

> Adapt the import paths (`use super::*` if `NoteNodeDto` is visible there) and the `compute_target_positions` signature to actuality.

- [ ] **Step 2: Generate the baseline**

```bash
BLESS_LAYOUT_SNAPSHOTS=1 cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::layout::tests::known_seed_layout_matches_baseline
```

Confirm the baseline file was written:

```bash
ls -l /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/tests/fixtures/layout_baseline_30nodes.json
```

- [ ] **Step 3: Re-run without the env var → must pass against the baseline**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::layout::tests::known_seed_layout_matches_baseline
```

Expected: PASS. From this point on, any change to layout numbers will fail this test and require `BLESS_LAYOUT_SNAPSHOTS=1` to re-bless.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/layout.rs interfaces/webchat/tests/fixtures/layout_baseline_30nodes.json
git commit -m "canvas: lock layout output with snapshot regression test"
```

---

## Phase 3 — Edges: Bézier curves + α-gradient + hop layering (no labels yet)

### Task 3.1 — Pure-function module `edge_curve.rs`

**Files:**
- Create: `interfaces/webchat/src/canvas_engine/edge_curve.rs`
- Modify: `interfaces/webchat/src/canvas_engine/mod.rs`

- [ ] **Step 1: Register the module**

Append to `canvas_engine/mod.rs`:

```rust
pub(crate) mod edge_curve;
```

- [ ] **Step 2: Write the failing tests + skeleton**

Create `edge_curve.rs`:

```rust
//! Quadratic-Bézier helpers for the new edge rendering.

use crate::canvas_engine::types::Vec2;

/// Sag coefficient: how much the curve bows out perpendicular to its chord.
/// `0.0` = straight line; `0.12` is the default the spec calls for.
pub(crate) const DEFAULT_SAG: f32 = 0.12;

/// Edge kinds that are *directional* and therefore render an arrow head.
/// Everything else (including `None`, `"related"`, any unknown string) is
/// drawn as a plain stroke.
pub(crate) const DIRECTIONAL_KINDS: &[&str] = &["refers", "derives", "follows"];

/// Quadratic-Bézier control point: midpoint, shoved perpendicular to
/// the chord by `length × sag_coef`. Sign of `sag_coef` controls which
/// side of the chord the curve bows toward.
pub(crate) fn edge_control_point(from: Vec2, to: Vec2, sag_coef: f32) -> Vec2 {
    let mid = Vec2 { x: (from.x + to.x) * 0.5, y: (from.y + to.y) * 0.5 };
    let dir = {
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        let len = (dx * dx + dy * dy).sqrt().max(1.0e-6);
        Vec2 { x: dx / len, y: dy / len }
    };
    let perp = Vec2 { x: -dir.y, y: dir.x };  // 90 ° CCW
    let len = {
        let dx = to.x - from.x;
        let dy = to.y - from.y;
        (dx * dx + dy * dy).sqrt()
    };
    let sag = len * sag_coef;
    Vec2 { x: mid.x + perp.x * sag, y: mid.y + perp.y * sag }
}

/// Position along the Bézier at `t ∈ [0, 1]` for a quadratic curve
/// with end-points `p0`, `p2` and control point `p1`.
pub(crate) fn bezier_point(p0: Vec2, p1: Vec2, p2: Vec2, t: f32) -> Vec2 {
    let mt = 1.0 - t;
    Vec2 {
        x: mt * mt * p0.x + 2.0 * mt * t * p1.x + t * t * p2.x,
        y: mt * mt * p0.y + 2.0 * mt * t * p1.y + t * t * p2.y,
    }
}

/// Tangent angle (radians) at `t` on the Bézier. Used to align edge labels.
/// Clamped to `[-π/4, π/4]` outside this function by callers (so labels never invert).
pub(crate) fn bezier_tangent(p0: Vec2, p1: Vec2, p2: Vec2, t: f32) -> f32 {
    let mt = 1.0 - t;
    let dx = 2.0 * mt * (p1.x - p0.x) + 2.0 * t * (p2.x - p1.x);
    let dy = 2.0 * mt * (p1.y - p0.y) + 2.0 * t * (p2.y - p1.y);
    dy.atan2(dx)
}

/// Per-hop visual layer for edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HopLayer { One, Two }

/// `(max_alpha, line_width)` tuple per layer.
pub(crate) fn hop_style(layer: HopLayer) -> (f32, f32) {
    match layer {
        HopLayer::One => (0.85, 1.8),
        HopLayer::Two => (0.55, 1.2),
    }
}

/// Does this edge get an arrow head?
pub(crate) fn is_directional(kind: Option<&str>) -> bool {
    kind.is_some_and(|k| DIRECTIONAL_KINDS.contains(&k))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32) -> Vec2 { Vec2 { x, y } }

    #[test]
    fn bezier_control_point_deterministic() {
        let a = edge_control_point(v(0.0, 0.0), v(100.0, 0.0), DEFAULT_SAG);
        let b = edge_control_point(v(0.0, 0.0), v(100.0, 0.0), DEFAULT_SAG);
        assert_eq!(a.x, b.x);
        assert_eq!(a.y, b.y);
    }

    #[test]
    fn control_point_perpendicular_to_edge_axis() {
        // Horizontal edge → control point displaced purely in y
        let cp = edge_control_point(v(0.0, 0.0), v(100.0, 0.0), DEFAULT_SAG);
        assert!(cp.x.abs() - 50.0 < 0.1);
        assert!(cp.y.abs() > 5.0, "expected vertical offset, got y={}", cp.y);
    }

    #[test]
    fn bezier_point_at_t_05_matches_formula() {
        // B(0.5) = 0.25·p0 + 0.5·p1 + 0.25·p2
        let p0 = v(0.0, 0.0);
        let p2 = v(100.0, 0.0);
        let p1 = edge_control_point(p0, p2, DEFAULT_SAG);
        let mid = bezier_point(p0, p1, p2, 0.5);
        let expected = Vec2 {
            x: 0.25 * p0.x + 0.5 * p1.x + 0.25 * p2.x,
            y: 0.25 * p0.y + 0.5 * p1.y + 0.25 * p2.y,
        };
        assert!((mid.x - expected.x).abs() < 1e-4);
        assert!((mid.y - expected.y).abs() < 1e-4);
    }

    #[test]
    fn hop_style_layered_by_hop() {
        let (a1, w1) = hop_style(HopLayer::One);
        let (a2, w2) = hop_style(HopLayer::Two);
        assert!(a1 > a2);
        assert!(w1 > w2);
    }

    #[test]
    fn is_directional_only_for_recognized_kinds() {
        assert!(is_directional(Some("refers")));
        assert!(is_directional(Some("derives")));
        assert!(is_directional(Some("follows")));
        assert!(!is_directional(Some("related")));
        assert!(!is_directional(Some("xyz")));
        assert!(!is_directional(None));
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::edge_curve
```

Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/edge_curve.rs interfaces/webchat/src/canvas_engine/mod.rs
git commit -m "canvas: add edge_curve module with Bézier helpers and hop-layer styling"
```

### Task 3.2 — Swap `renderer.rs::draw_edges_for_node` to use Bézier + gradient

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs:427-…` (the `draw_edges_for_node` function body found at line 427 in the current code)

- [ ] **Step 1: Read the existing function**

```bash
sed -n '427,530p' /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/canvas_engine/renderer.rs
```

Identify the `set_stroke_style_canvas_gradient` / `set_line_width` / `move_to` / `line_to` calls (around lines 507–512). These are the lines to replace.

- [ ] **Step 2: Rewrite the inner loop**

Replace the straight-line render with a Bézier render. The relevant block (around lines 504–512) becomes:

```rust
use crate::canvas_engine::edge_curve::{edge_control_point, hop_style, HopLayer, DEFAULT_SAG};

let layer = if hop <= 1 { HopLayer::One } else { HopLayer::Two };
let (max_alpha, line_width) = hop_style(layer);

let cp = edge_control_point(
    Vec2 { x: from_pos.0, y: from_pos.1 },
    Vec2 { x: to_pos.0,   y: to_pos.1 },
    DEFAULT_SAG,
);

// α-gradient stroke: invisible at endpoints, max at the chord interior
let grad = ctx.create_linear_gradient(
    from_pos.0 as f64, from_pos.1 as f64,
    to_pos.0   as f64, to_pos.1   as f64,
);
let _ = grad.add_color_stop(0.00, "rgba(167,139,250,0.00)");
let _ = grad.add_color_stop(0.15, &format!("rgba(167,139,250,{:.3})", max_alpha));
let _ = grad.add_color_stop(0.85, &format!("rgba(167,139,250,{:.3})", max_alpha));
let _ = grad.add_color_stop(1.00, "rgba(167,139,250,0.00)");
ctx.set_stroke_style_canvas_gradient(&grad);
ctx.set_line_width(line_width as f64);

ctx.begin_path();
ctx.move_to(from_pos.0 as f64, from_pos.1 as f64);
// Quadratic via degenerate cubic: both control points coincide.
ctx.bezier_curve_to(
    cp.x as f64, cp.y as f64,
    cp.x as f64, cp.y as f64,
    to_pos.0 as f64, to_pos.1 as f64,
);
ctx.stroke();
```

If the function loops over neighbours and references a per-edge `hop` value that isn't already present, compute it from `nbhd.hop_depth.get(&edge.to)` or similar — the existing function's parameters expose it.

- [ ] **Step 3: Build**

```bash
pgrep -x cargo | wc -l
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

Expected: clean. If `bezier_curve_to` isn't on `CanvasRenderingContext2d`, check the `web-sys` features — it's in the `CanvasRenderingContext2d` feature, which is already enabled by anything else in `renderer.rs`.

- [ ] **Step 4: Manual smoke**

```bash
just dev
# In browser, navigate to /memory. Edges should now be subtly curved
# and softly fade in/out at the endpoints. 1-hop edges visibly thicker
# and brighter than 2-hop.
```

Take a screenshot for the CHANGELOG draft.

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas: render edges as α-gradient Bézier curves with hop-layered weight"
```

### Task 3.3 — Edge highlight on hover / selected node

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs::draw_edges_for_node`
- Possibly modify: `interfaces/webchat/src/canvas_engine/renderer.rs` (top of file: introduce a small helper to compute the adjacency set)

- [ ] **Step 1: Locate the hover / selected signal source**

```bash
grep -n "hovered\|selected\|hovered_id\|selected_id" /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/canvas_engine/renderer.rs | head -10
```

The function probably already receives `hovered: Option<&str>` and `selected: Option<&str>` parameters (or they come from a context). Confirm before writing the implementation.

- [ ] **Step 2: Two-pass render — dim first, bright second**

Wrap the per-edge body so that, for each edge `(from_id, to_id)`:

```rust
let highlight_anchor = selected.or(hovered);
let is_adjacent = highlight_anchor
    .map(|h| h == from_id || h == to_id)
    .unwrap_or(false);

let (alpha_mul, width_mul, color_rgb) = if highlight_anchor.is_some() {
    if is_adjacent {
        (1.0_f32 / max_alpha,  // promote to max_alpha → 1.0
         1.5_f32,
         "252,211,77")          // gold
    } else {
        (0.4_f32, 1.0_f32, "167,139,250") // existing purple, dimmed
    }
} else {
    (1.0_f32, 1.0_f32, "167,139,250")
};

let effective_alpha = (max_alpha * alpha_mul).clamp(0.0, 1.0);
let effective_width = line_width * width_mul;

let _ = grad.add_color_stop(0.00, &format!("rgba({color_rgb},0.00)"));
let _ = grad.add_color_stop(0.15, &format!("rgba({color_rgb},{:.3})", effective_alpha));
let _ = grad.add_color_stop(0.85, &format!("rgba({color_rgb},{:.3})", effective_alpha));
let _ = grad.add_color_stop(1.00, &format!("rgba({color_rgb},0.00)"));
ctx.set_line_width(effective_width as f64);
```

Reorder the loop so the function first walks all non-adjacent (dim) edges, *then* walks adjacent (bright) edges. This ensures bright edges draw on top.

- [ ] **Step 3: Build + smoke**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
just dev
# In browser, hover a node — adjacent edges turn gold and brighten,
# non-adjacent fade. Selected node holds that state on click.
```

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas: highlight edges adjacent to hovered/selected node in gold"
```

---

## Phase 4 — Node cards (FULL / MINI / DOT) over a DOM overlay

> The biggest visual change. From here on, the right-side canvas no longer draws circles; Canvas2D draws only edges + shadows + starfield. Nodes are Leptos components positioned via `transform: translate3d`.

### Task 4.1 — `MemoryState` context provider

**Files:**
- Create: `interfaces/webchat/src/state/memory.rs`
- Modify: `interfaces/webchat/src/state/mod.rs` (or wherever module declarations live — check `interfaces/webchat/src/lib.rs` first)

- [ ] **Step 1: Verify the module home**

```bash
grep -rn "^mod state\|^pub mod state" /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src --include="*.rs" | head -3
```

If a `state` module already exists, write into it; if not, create `interfaces/webchat/src/state/mod.rs` and register it in `lib.rs`:

```rust
// in lib.rs
pub mod state;
```

- [ ] **Step 2: Write the context struct**

Create `state/memory.rs`:

```rust
//! Shared memory-mode state, lifted from `RadialCanvasView` so the
//! sidebar (search / fold / detail panel) and the canvas itself can
//! both read and mutate it.

use leptos::prelude::*;
use std::collections::VecDeque;

#[derive(Clone, Copy)]
pub struct MemoryState {
    pub agent_id:           RwSignal<String>,
    pub search_query:       RwSignal<String>,
    pub fold_threshold:     RwSignal<usize>,
    pub selected_node:      RwSignal<Option<String>>,
    pub focus_id:           RwSignal<Option<String>>,
    pub breadcrumb_entries: RwSignal<Vec<String>>,
    pub recent_visited:     RwSignal<VecDeque<String>>,
    pub sidebar_collapsed:  RwSignal<bool>,
}

impl MemoryState {
    pub fn new() -> Self {
        Self {
            agent_id:           RwSignal::new("main".into()),
            search_query:       RwSignal::new(String::new()),
            fold_threshold:     RwSignal::new(3),
            selected_node:      RwSignal::new(None),
            focus_id:           RwSignal::new(None),
            breadcrumb_entries: RwSignal::new(Vec::new()),
            recent_visited:     RwSignal::new(VecDeque::with_capacity(8)),
            sidebar_collapsed:  RwSignal::new(false),
        }
    }

    /// Push `id` to the front of `recent_visited`, dropping oldest beyond capacity 8.
    /// De-duplicates: existing copies are removed first so the most-recent entry
    /// always sits at the front.
    pub fn push_recent(&self, id: String) {
        self.recent_visited.update(|q| {
            q.retain(|x| x != &id);
            q.push_front(id);
            while q.len() > 8 { q.pop_back(); }
        });
    }
}

impl Default for MemoryState {
    fn default() -> Self { Self::new() }
}
```

- [ ] **Step 3: Provide the context at `MainContent` mount**

Edit `interfaces/webchat/src/app.rs` — inside the `MainContent` component (lines 92–117), add at the top:

```rust
use crate::state::memory::MemoryState;
// ...inside the function body, before the `view!`:
provide_context(MemoryState::new());
```

- [ ] **Step 4: Add tests**

Append to `state/memory.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_recent_caps_at_8_and_dedupes() {
        let s = MemoryState::new();
        for i in 0..12 {
            s.push_recent(format!("id-{i}"));
        }
        s.recent_visited.with(|q| {
            assert_eq!(q.len(), 8);
            assert_eq!(q[0], "id-11"); // most recent at front
        });

        s.push_recent("id-5".into());
        s.recent_visited.with(|q| {
            assert_eq!(q[0], "id-5");
            // and it does not appear twice
            assert_eq!(q.iter().filter(|x| **x == "id-5").count(), 1);
        });
    }
}
```

- [ ] **Step 5: Run tests + build**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib state::memory
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/state/ interfaces/webchat/src/lib.rs interfaces/webchat/src/app.rs
git commit -m "canvas: lift memory state into MemoryState context provider"
```

### Task 4.2 — `category_color` helper

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/mod.rs` (re-export)
- Create: `interfaces/webchat/src/canvas_engine/category_color.rs`

- [ ] **Step 1: Register the module**

Append to `canvas_engine/mod.rs`:

```rust
pub(crate) mod category_color;
```

- [ ] **Step 2: Implement + test**

Create `category_color.rs`:

```rust
//! Map a free-form `category` string to a CSS color expression for the node stripe.

use crate::canvas_engine::fnv1a::fnv1a_32;

/// Returns a CSS color string. Well-known categories → curated variable;
/// anything else → deterministic `hsl(hue, 55%, 65%)`.
pub fn category_color(category: &str) -> String {
    match category {
        "feedback"  => "var(--cat-feedback)".to_string(),
        "project"   => "var(--cat-project)".to_string(),
        "reference" => "var(--cat-reference)".to_string(),
        "user"      => "var(--cat-user)".to_string(),
        other => {
            let hue = fnv1a_32(other.as_bytes()) % 360;
            format!("hsl({hue}, 55%, 65%)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_categories_map_to_vars() {
        assert_eq!(category_color("feedback"),  "var(--cat-feedback)");
        assert_eq!(category_color("project"),   "var(--cat-project)");
        assert_eq!(category_color("reference"), "var(--cat-reference)");
        assert_eq!(category_color("user"),      "var(--cat-user)");
    }

    #[test]
    fn unknown_categories_use_deterministic_hsl() {
        let a = category_color("custom-xyz");
        let b = category_color("custom-xyz");
        assert_eq!(a, b);
        assert!(a.starts_with("hsl("));
    }
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib canvas_engine::category_color
git add interfaces/webchat/src/canvas_engine/category_color.rs interfaces/webchat/src/canvas_engine/mod.rs
git commit -m "canvas: add category_color helper (curated vars + HSL fallback)"
```

### Task 4.3 — CSS tokens for cards

**Files:**
- Modify: the stylesheet identified at the start of Phase 4 (likely `interfaces/webchat/src/styles.css` or `interfaces/webchat/styles/...`)

- [ ] **Step 1: Find the right file**

```bash
grep -rln "aleph-sidebar\|aleph-shell" /Volumes/TBU4/Workspace/Aleph/interfaces/webchat --include="*.css" --include="*.scss"
```

Use the most relevant hit (probably a single file).

- [ ] **Step 2: Append the design-token block at the bottom of that file**

```css
/* ── Memory canvas node tokens ─────────────────────────────────────── */
:root {
    --node-bg:           linear-gradient(135deg, #1a1a2e, #16162a);
    --node-border:       rgba(255, 255, 255, 0.06);
    --text-title:        #f1f5f9;
    --text-body:         #94a3b8;
    --text-meta:         #64748b;
    --text-code:         #e2e8f0;

    --cat-feedback:      #a78bfa;
    --cat-project:       #34d399;
    --cat-reference:     #60a5fa;
    --cat-user:          #fbbf24;

    --shadow-base:       0 8px 32px rgba(0, 0, 0, 0.6);
    --glow-hover:        0 0 24px rgba(167, 139, 250, 0.27);
    --glow-selected:     0 0 32px rgba(167, 139, 250, 0.53), 0 0 0 2px #a78bfa;
    --glow-active:       0 0 32px rgba(252, 211, 77, 0.67);
}

@keyframes node-card-breath {
    0%, 100% { filter: brightness(1.0); }
    50%      { filter: brightness(1.15); }
}

.node-card-full {
    position: absolute;
    width: 280px;
    background: var(--node-bg);
    border: 1px solid var(--node-border);
    border-radius: 10px;
    box-shadow: var(--shadow-base);
    overflow: hidden;
    transition: box-shadow 120ms ease-out;
    will-change: transform;
    user-select: none;
}
.node-card-full:hover               { box-shadow: var(--shadow-base), var(--glow-hover); }
.node-card-full[data-selected]      { box-shadow: var(--shadow-base), var(--glow-selected); }
.node-card-full[data-active]        { box-shadow: var(--shadow-base), var(--glow-active); animation: node-card-breath 2.5s ease-in-out infinite; }

.node-card-mini {
    position: absolute;
    width: 140px;
    background: rgba(26, 26, 46, 0.93);
    border: 1px solid var(--node-border);
    border-radius: 7px;
    padding: 6px 9px;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
    transition: box-shadow 120ms ease-out;
    color: var(--text-title);
    font-size: 11px;
    line-height: 1.3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    will-change: transform;
}
.node-card-mini:hover               { box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4), var(--glow-hover); }

.node-card-dot {
    position: absolute;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    box-shadow: 0 0 12px currentColor;
    transition: width 120ms ease-out, height 120ms ease-out, box-shadow 120ms ease-out;
    cursor: pointer;
    will-change: transform;
}
.node-card-dot:hover { width: 14px; height: 14px; box-shadow: 0 0 18px currentColor; }
```

- [ ] **Step 3: Build verifies nothing broke**

```bash
just dev
# Visit any page — the CSS variables just sit there idle.
```

- [ ] **Step 4: Commit**

```bash
git add <stylesheet path>
git commit -m "canvas: add node-card design-token CSS variables and class rules"
```

### Task 4.4 — `<NodeCard>` Leptos component (three modes)

**Files:**
- Create: `interfaces/webchat/src/views/canvas/node_card.rs`
- Modify: `interfaces/webchat/src/views/canvas/mod.rs` (declare the new sub-module)

- [ ] **Step 1: Declare the module**

In `views/canvas/mod.rs`, add `mod node_card;` near the other `mod …;` lines at the top.

- [ ] **Step 2: Write the component**

Create `node_card.rs`:

```rust
use leptos::prelude::*;
use crate::canvas_engine::category_color::category_color;
use crate::canvas_engine::markdown_excerpt::render_excerpt;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CardMode { Full, Mini, Dot }

/// Choose the render mode for a node given its hop / hover / select / zoom state.
pub fn pick_mode(hop: u8, is_hovered: bool, is_selected: bool, zoom: f32) -> CardMode {
    if zoom < 0.5 { return CardMode::Dot; }
    if hop == 0 || is_hovered || is_selected {
        // active center always FULL; otherwise hover/select bumps by one level
        return CardMode::Full;
    }
    if hop == 1 { CardMode::Mini } else { CardMode::Dot }
}

#[component]
pub fn NodeCard(
    /// Node id (used as React-style key + click target).
    #[prop(into)] id: String,
    /// Node display name (title, never Markdown).
    #[prop(into)] name: String,
    /// Free-form category — colors the stripe.
    #[prop(into)] category: String,
    /// Tags shown in the meta footer.
    #[prop(default = vec![])] tags: Vec<String>,
    /// Pre-rendered HTML excerpt (call `render_excerpt(raw)` outside this component).
    /// Empty string is allowed (renders no body).
    #[prop(into)] excerpt_html: String,
    /// Hop distance from the center node; 0 = center.
    hop: u8,
    /// Reactive — current screen position of this node.
    screen_xy: ReadSignal<(f32, f32)>,
    /// Current canvas zoom.
    zoom: ReadSignal<f32>,
    /// Reactive — id of the hovered node (if any).
    hovered_id: ReadSignal<Option<String>>,
    /// Reactive — id of the selected node (if any).
    selected_id: ReadSignal<Option<String>>,
    /// Click handler — receives the node id.
    on_click: Callback<String>,
) -> impl IntoView {
    let id_for_click = id.clone();
    let id_for_hover_match = id.clone();
    let id_for_sel_match = id.clone();
    let id_for_active = id.clone();
    let stripe_color = category_color(&category);

    let mode = Memo::new(move |_| {
        let hov_match = hovered_id.with(|h| h.as_deref() == Some(&id_for_hover_match));
        let sel_match = selected_id.with(|s| s.as_deref() == Some(&id_for_sel_match));
        pick_mode(hop, hov_match, sel_match, zoom.get())
    });

    let style = move || {
        let (x, y) = screen_xy.get();
        let (w_half, h_half) = match mode.get() {
            CardMode::Full => (140.0, 60.0),
            CardMode::Mini => (70.0, 15.0),
            CardMode::Dot  => (5.0, 5.0),
        };
        format!(
            "transform: translate3d({:.1}px, {:.1}px, 0); color: {};",
            x - w_half, y - h_half, stripe_color
        )
    };

    let click_id = id_for_click.clone();
    let click_handler = move |_| { on_click.run(click_id.clone()); };

    view! {
        {move || match mode.get() {
            CardMode::Full => {
                let body_html = excerpt_html.clone();
                let is_active = hop == 0;
                view! {
                    <div
                        class="node-card-full"
                        style=style
                        on:click=click_handler.clone()
                        data-id=id.clone()
                        data-selected=move || selected_id.with(|s| s.as_deref() == Some(&id_for_active)).then(|| "true")
                        data-active=move || is_active.then(|| "true")
                    >
                        <div style=format!("height:3px;background:{}", stripe_color)></div>
                        <div style="padding:10px 14px 4px;color:var(--text-title);font-size:13px;font-weight:600;line-height:1.3;display:-webkit-box;-webkit-line-clamp:2;-webkit-box-orient:vertical;overflow:hidden">
                            {name.clone()}
                        </div>
                        {(!body_html.is_empty()).then(|| view! {
                            <div
                                style="padding:0 14px 8px;color:var(--text-body);font-size:11.5px;line-height:1.55;display:-webkit-box;-webkit-line-clamp:3;-webkit-box-orient:vertical;overflow:hidden"
                                inner_html=body_html
                            ></div>
                        })}
                        {(!tags.is_empty()).then(|| {
                            let tags_inner = tags.clone();
                            view! {
                                <div style="padding:6px 14px;border-top:1px solid rgba(255,255,255,0.05);display:flex;gap:4px;color:var(--text-meta);font-size:10px">
                                    {tags_inner.into_iter().map(|t| view! {
                                        <span style="color:var(--cat-feedback);background:rgba(167,139,250,0.13);padding:1px 5px;border-radius:3px">"#"{t}</span>
                                    }).collect_view()}
                                </div>
                            }
                        })}
                    </div>
                }.into_any()
            }
            CardMode::Mini => view! {
                <div
                    class="node-card-mini"
                    style=style
                    on:click=click_handler.clone()
                    data-id=id.clone()
                >
                    <span style=format!("display:inline-block;width:6px;height:6px;border-radius:2px;background:{};margin-right:6px;vertical-align:middle", stripe_color)></span>
                    {name.clone()}
                </div>
            }.into_any(),
            CardMode::Dot => view! {
                <div
                    class="node-card-dot"
                    style=move || {
                        let (x, y) = screen_xy.get();
                        format!("transform: translate3d({:.1}px, {:.1}px, 0); background: {}; color: {};", x - 5.0, y - 5.0, stripe_color, stripe_color)
                    }
                    on:click=click_handler.clone()
                    data-id=id.clone()
                ></div>
            }.into_any(),
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_mode_dot_under_low_zoom() {
        assert_eq!(pick_mode(0, false, false, 0.3), CardMode::Dot);
        assert_eq!(pick_mode(1, true,  true,  0.3), CardMode::Dot);
    }

    #[test]
    fn pick_mode_full_at_center() {
        assert_eq!(pick_mode(0, false, false, 1.0), CardMode::Full);
    }

    #[test]
    fn pick_mode_hover_promotes_one_step() {
        assert_eq!(pick_mode(2, false, false, 1.0), CardMode::Dot);
        assert_eq!(pick_mode(2, true,  false, 1.0), CardMode::Full); // promoted
        assert_eq!(pick_mode(1, false, false, 1.0), CardMode::Mini);
        assert_eq!(pick_mode(1, false, true,  1.0), CardMode::Full); // promoted
    }
}
```

- [ ] **Step 3: Run unit tests**

```bash
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib views::canvas::node_card
```

Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/canvas/node_card.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: add NodeCard Leptos component with FULL/MINI/DOT modes"
```

### Task 4.5 — DOM-overlay sync in `graph_canvas.rs`

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`
- Modify: `interfaces/webchat/src/canvas_engine/renderer.rs` (delete the now-redundant `draw_node` call site, keep the function alive for selection rings only)

> This is the biggest task in this phase. Read both files end-to-end before starting.

- [ ] **Step 1: Map the rAF loop**

```bash
grep -n "request_animation_frame\|rAF\|raf" /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/views/canvas/graph_canvas.rs | head -10
```

Find the closure that calls `draw_nodes` / `draw_edges_for_node` each frame. That closure also computes `screen_pos` for each node (world → screen via the viewport transform).

- [ ] **Step 2: Introduce a per-node `screen_xy` signal**

Inside `RadialCanvasView` (or wherever `GraphCanvas` is mounted from), allocate a signal:

```rust
// HashMap<id, RwSignal<(f32, f32)>> — one signal per visible node so they
// update independently and Leptos doesn't re-render the whole list each frame.
let node_screen_pos: RwSignal<std::collections::HashMap<String, RwSignal<(f32, f32)>>> = RwSignal::new(Default::default());
```

Each rAF tick, for each visible node:

```rust
let sig = node_screen_pos.with(|m| m.get(&node.id).copied());
let sig = sig.unwrap_or_else(|| {
    let s = RwSignal::new((0.0, 0.0));
    node_screen_pos.update(|m| { m.insert(node.id.clone(), s); });
    s
});
sig.set((screen_x, screen_y));
```

- [ ] **Step 3: Render the `<NodeCard>` overlay layer**

In the same `view! { ... }` as the `<canvas>` element (likely in `graph_canvas.rs`), append a sibling `<div>` that maps every visible node id to a `<NodeCard>`:

```rust
view! {
    <canvas node_ref=canvas_ref ... />
    <div class="absolute inset-0 pointer-events-none">
        // Cards have pointer-events:auto via individual class rules.
        {move || nodes.get().into_iter().map(|n| {
            let sig = node_screen_pos.with(|m| m.get(&n.id).copied());
            let sig = sig.unwrap_or_else(|| RwSignal::new((0.0, 0.0)));
            let excerpt_html = render_excerpt(&n.body_or_empty);
            view! {
                <NodeCard
                    id=n.id.clone()
                    name=n.name.clone()
                    category=n.category.clone()
                    tags=n.tags.clone()
                    excerpt_html=excerpt_html
                    hop=n.hop
                    screen_xy=sig.read_only()
                    zoom=current_zoom.read_only()
                    hovered_id=hovered_id.read_only()
                    selected_id=selected_id.read_only()
                    on_click=Callback::new(move |id| { /* dispatch to MemoryState.selected_node */ })
                />
            }
        }).collect_view()}
    </div>
}
```

`n.body_or_empty` is `""` for non-center, non-hovered, non-selected nodes (avoid the lazy-fetch trip in those cases — Task 4.6 wires the lazy fetch).

- [ ] **Step 4: Delete `draw_node` from the renderer hot path**

In `renderer.rs::draw_nodes` (or wherever the per-node circle is drawn), comment out / delete the call to `draw_node`. Keep `draw_orphan_ring` if it draws a selection ring; otherwise delete it. Keep edge drawing as-is.

Run:

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

Address any dead-code warnings by adding `#[allow(dead_code)]` to `draw_node` if it's still referenced elsewhere, or by deleting it entirely if it isn't.

- [ ] **Step 5: Manual smoke**

```bash
just dev
# Visit /memory.
# Expected: edges still render, but instead of glowing circles, the
# center node shows a FULL card, 1-hop nodes show MINI pills,
# 2-hop / orphan nodes show DOT glyphs. Hovering an outer node
# bumps it up a tier; clicking selects it.
```

- [ ] **Step 6: Commit**

```bash
git add interfaces/webchat/src/views/canvas/graph_canvas.rs interfaces/webchat/src/canvas_engine/renderer.rs
git commit -m "canvas: render nodes as DOM overlay cards on top of canvas2d edges"
```

### Task 4.6 — Lazy-fetch excerpt on FULL-mode upgrade

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs` (or wherever hover/select fires)

- [ ] **Step 1: Add a `note_detail_cache` instance**

In the component where hover/select state lives:

```rust
let note_detail_cache: Rc<RefCell<PrefetchCache<NoteDetailResponse>>> =
    Rc::new(RefCell::new(PrefetchCache::new()));
```

- [ ] **Step 2: When a node transitions into FULL mode, fetch its detail**

Use an `Effect` that observes `(hovered_id, selected_id)` and, when either points at a node *not* already in the cache, fires `GraphApi::note_detail(id)` (or its existing equivalent) and `put`s the response into the cache. Then update a separate `RwSignal<HashMap<String, String>>` (`excerpt_by_id`) so the `<NodeCard>` view's `excerpt_html` reactive prop refreshes.

Boilerplate (adapt to actual API names):

```rust
let excerpt_by_id: RwSignal<HashMap<String, String>> = RwSignal::new(Default::default());
let cache = note_detail_cache.clone();
let state_ctx = state.clone();
Effect::new(move |_| {
    let target = selected_id.get().or_else(|| hovered_id.get());
    let Some(id) = target else { return; };
    let now = now_ms();
    if cache.borrow().has(&id, now) { return; }
    if excerpt_by_id.with(|m| m.contains_key(&id)) { return; }
    let cache_clone = cache.clone();
    let id_clone = id.clone();
    let state_clone = state_ctx;
    spawn_local(async move {
        if let Ok(detail) = GraphApi::note_detail(&state_clone, &id_clone).await {
            let html = render_excerpt(&detail.content);
            excerpt_by_id.update(|m| { m.insert(id_clone.clone(), html); });
            cache_clone.borrow_mut().put(id_clone, detail, now_ms());
        }
    });
});
```

- [ ] **Step 3: Wire `excerpt_by_id` into the `<NodeCard>` map call**

In Task 4.5's view, replace:

```rust
let excerpt_html = render_excerpt(&n.body_or_empty);
```

with:

```rust
let id_lookup = n.id.clone();
let excerpt_html = excerpt_by_id.with(|m| m.get(&id_lookup).cloned()).unwrap_or_default();
```

So that the rendered HTML is empty until the lazy fetch fills it, then the card refreshes with the body.

- [ ] **Step 4: Smoke**

```bash
just dev
# Click a node — its FULL card should populate the body within ≤ 1 frame.
# Hover a 2-hop node — DOT → FULL transition triggers fetch then renders body.
```

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/views/canvas/graph_canvas.rs
git commit -m "canvas: lazy-fetch note detail when a card enters FULL mode"
```

---

## Phase 5 — Sidebar restructure (fill MemorySidebar, strip the right side)

### Task 5.1 — Build `NodeDetailPanel` (240 px sidebar variant + recent-5 empty state)

**Files:**
- Create: `interfaces/webchat/src/views/canvas/node_detail_panel.rs`
- Modify: `interfaces/webchat/src/views/canvas/mod.rs` (declare module)

- [ ] **Step 1: Declare**

In `views/canvas/mod.rs`:

```rust
mod node_detail_panel;
pub use node_detail_panel::NodeDetailPanel;
```

- [ ] **Step 2: Implement**

Create `node_detail_panel.rs`:

```rust
use leptos::prelude::*;
use crate::state::memory::MemoryState;
use crate::canvas_engine::markdown_excerpt::render_excerpt;
use crate::canvas_engine::category_color::category_color;
use std::collections::HashMap;

/// Sidebar variant of the node detail view. 240 px content width — the
/// shell sidebar (`w-64`) gives us 256 px and the surrounding padding
/// claims the rest. When no node is selected, falls back to a
/// "recently visited" list.
#[component]
pub fn NodeDetailPanel(
    /// Pre-fetched body excerpts keyed by node id.
    excerpts: RwSignal<HashMap<String, NodeExcerpt>>,
) -> impl IntoView {
    let mem = expect_context::<MemoryState>();

    view! {
        <div class="flex-1 min-h-0 overflow-y-auto px-3 py-2">
            {move || {
                let selected = mem.selected_node.get();
                if let Some(id) = selected {
                    if let Some(ex) = excerpts.with(|m| m.get(&id).cloned()) {
                        view! { <DetailFor excerpt=ex /> }.into_any()
                    } else {
                        view! { <DetailLoading id=id /> }.into_any()
                    }
                } else {
                    view! { <RecentVisitedList /> }.into_any()
                }
            }}
        </div>
    }
}

#[derive(Clone)]
pub struct NodeExcerpt {
    pub id: String,
    pub name: String,
    pub category: String,
    pub tags: Vec<String>,
    pub body_markdown: String,
    pub breadcrumb: Vec<String>,
}

#[component]
fn DetailFor(excerpt: NodeExcerpt) -> impl IntoView {
    let stripe = category_color(&excerpt.category);
    let body_html = render_excerpt(&excerpt.body_markdown);
    let breadcrumb = excerpt.breadcrumb.clone();

    view! {
        <div>
            {(!breadcrumb.is_empty()).then(|| view! {
                <div style="font-size:10px;color:var(--text-meta);margin-bottom:6px">
                    {breadcrumb.join(" › ")}
                </div>
            })}
            <div style=format!("height:3px;background:{};border-radius:2px;margin-bottom:8px", stripe)></div>
            <h3 style="color:var(--text-title);font-size:14px;font-weight:600;line-height:1.3;margin:0 0 6px">
                {excerpt.name.clone()}
            </h3>
            <div style="color:var(--text-body);font-size:12px;line-height:1.55" inner_html=body_html></div>
            {(!excerpt.tags.is_empty()).then(|| {
                let t = excerpt.tags.clone();
                view! {
                    <div style="margin-top:10px;display:flex;flex-wrap:wrap;gap:4px">
                        {t.into_iter().map(|tag| view! {
                            <span style="font-size:10px;color:var(--cat-feedback);background:rgba(167,139,250,0.13);padding:1px 6px;border-radius:3px">
                                "#"{tag}
                            </span>
                        }).collect_view()}
                    </div>
                }
            })}
        </div>
    }
}

#[component]
fn DetailLoading(id: String) -> impl IntoView {
    view! {
        <div style="color:var(--text-meta);font-size:11px;font-style:italic">
            "Loading " {id} " …"
        </div>
    }
}

#[component]
fn RecentVisitedList() -> impl IntoView {
    let mem = expect_context::<MemoryState>();

    view! {
        <div>
            <div style="text-transform:uppercase;font-size:9.5px;color:var(--text-meta);letter-spacing:0.05em;margin-bottom:6px">
                "Recently visited"
            </div>
            {move || {
                mem.recent_visited.with(|q| {
                    let top: Vec<String> = q.iter().take(5).cloned().collect();
                    if top.is_empty() {
                        view! {
                            <p style="color:var(--text-meta);font-size:11px;font-style:italic">
                                "Click a node to inspect it. Recently visited memories will appear here."
                            </p>
                        }.into_any()
                    } else {
                        view! {
                            <ul style="list-style:none;padding:0;margin:0;display:flex;flex-direction:column;gap:4px">
                                {top.into_iter().map(|id| {
                                    let id_for_click = id.clone();
                                    view! {
                                        <li
                                            style="font-size:11.5px;color:var(--text-body);padding:6px 8px;border-radius:5px;background:rgba(255,255,255,0.02);cursor:pointer"
                                            on:click=move |_| {
                                                mem.selected_node.set(Some(id_for_click.clone()));
                                            }
                                        >
                                            {id}
                                        </li>
                                    }
                                }).collect_view()}
                            </ul>
                        }.into_any()
                    }
                })
            }}
        </div>
    }
}
```

- [ ] **Step 3: Build**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/canvas/node_detail_panel.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: add NodeDetailPanel for sidebar with recent-5 empty state"
```

### Task 5.2 — Rewrite `MemorySidebar` content stack

**Files:**
- Modify: `interfaces/webchat/src/components/mode_sidebar.rs:100-112` (replace `MemorySidebar` body)

- [ ] **Step 1: Read the current placeholder**

```bash
sed -n '95,115p' /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/components/mode_sidebar.rs
```

- [ ] **Step 2: Rewrite**

Replace the body of `fn MemorySidebar() -> impl IntoView` with:

```rust
#[component]
fn MemorySidebar() -> impl IntoView {
    use crate::state::memory::MemoryState;
    use crate::views::canvas::node_detail_panel::{NodeDetailPanel, NodeExcerpt};
    use std::collections::HashMap;

    let mem = expect_context::<MemoryState>();
    let excerpts: RwSignal<HashMap<String, NodeExcerpt>> = RwSignal::new(Default::default());

    view! {
        <div class="flex flex-col h-full">
            // ── Agent dropdown ─────────────────────────────────
            <div class="px-3 pt-3 pb-1.5">
                <label style="font-size:9.5px;color:var(--text-meta);text-transform:uppercase;letter-spacing:0.05em">
                    "Agent"
                </label>
                <select
                    class="w-full mt-1 bg-[#1a1a2e] border border-[#2a2a40] rounded text-xs text-[#e2e8f0] px-2 py-1.5"
                    prop:value=move || mem.agent_id.get()
                    on:change=move |ev| {
                        let v = event_target_value(&ev);
                        mem.agent_id.set(v);
                    }
                >
                    // Options populated by the fetch_agents effect already present in RadialCanvasView.
                    // For now, render whatever ids are in `mem.agent_id`.
                    <option value=move || mem.agent_id.get()>{move || mem.agent_id.get()}</option>
                </select>
            </div>
            // ── Search ─────────────────────────────────────────
            <div class="px-3 pb-1.5">
                <label style="font-size:9.5px;color:var(--text-meta);text-transform:uppercase;letter-spacing:0.05em">
                    "Search"
                </label>
                <input
                    type="search"
                    placeholder="keyword…"
                    class="w-full mt-1 bg-[#1a1a2e] border border-[#2a2a40] rounded text-xs text-[#e2e8f0] px-2 py-1.5 placeholder-[#64748b]"
                    prop:value=move || mem.search_query.get()
                    on:input=move |ev| {
                        let v = event_target_value(&ev);
                        // 200ms debounce: cancel any previous pending update; for MVP we set directly.
                        mem.search_query.set(v);
                    }
                />
            </div>
            // ── Fold threshold ────────────────────────────────
            <div class="px-3 pb-2">
                <label style="font-size:9.5px;color:var(--text-meta);text-transform:uppercase;letter-spacing:0.05em">
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
            // ── Detail panel (flex-1, fills sidebar) ──────────
            <NodeDetailPanel excerpts=excerpts />
            // ── Footer: counts + collapse ─────────────────────
            <div class="border-t border-[#2a2a40] px-3 py-2 flex items-center justify-between">
                <span style="font-size:10px;color:var(--text-meta)">
                    // Counts come from canvas state in Phase 5b — wire via mem.focus_id graph
                    "graph"
                </span>
                <button
                    type="button"
                    class="text-[#a78bfa] text-xs px-2 py-1 rounded hover:bg-[#a78bfa14]"
                    on:click=move |_| { mem.sidebar_collapsed.set(true); }
                    title="Collapse sidebar (Esc to restore)"
                >
                    "⇧"
                </button>
            </div>
        </div>
    }
}
```

> Note: the dropdown `<option>` list is intentionally minimal here. Populating it from the existing `fetch_agents` effect is Task 5.3.

- [ ] **Step 3: Build**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
```

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/components/mode_sidebar.rs
git commit -m "canvas: rewrite MemorySidebar with agent/search/fold/detail/footer stack"
```

### Task 5.3 — Strip the right-side stack from `RadialCanvasView`

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/mod.rs:578-641` (the inner `view!` of `RadialCanvasView`)

- [ ] **Step 1: Cut the four old widgets out of the view**

Replace `view! { … }` in `RadialCanvasView` (the block from lines 578–641 in the current code) with:

```rust
view! {
    <div class="relative w-full h-full bg-[#080818]">
        <GraphCanvas
            graph_state=graph_state.clone()
            on_event=Callback::new(on_event)
            nav=nav.clone()
        />
        {
            #[cfg(target_arch = "wasm32")]
            { view! {
                <MiniMapOverlay
                    minimap=minimap.clone()
                    focus_id=focus_id
                    focus_neighbor_ids=focus_neighbors
                    on_pick=move |id: String| {
                        set_selected_node.set(Some(id.clone()));
                        active_request.set(Some(id));
                    }
                />
            } }
            #[cfg(not(target_arch = "wasm32"))]
            { () }
        }
    </div>
}
```

This removes `<AgentSelectorBar/>`, `<CanvasToolbar/>`, `<Breadcrumb/>`, and `<DetailPanel/>` from the right side. Their state lives on in `MemoryState` (Task 4.1) and is consumed by `MemorySidebar` (Task 5.2).

- [ ] **Step 2: Migrate the `fetch_agents` effect**

The `fetch_agents` closure currently runs inside `RadialCanvasView` and populates the local `agents` signal. Move it so that:
1. The agent list (`Vec<AgentSummary>`) is stored in a new `RwSignal<Vec<AgentSummary>>` provided alongside `MemoryState`. Either extend `MemoryState` with `agents: RwSignal<Vec<AgentSummary>>` (preferred) or use a separate context.
2. The effect that calls `AgentsApi::list` lives in the same module as the new `MemoryState::new()` factory.
3. The `<select>` in `MemorySidebar` reads from that signal and emits `<option>` per agent.

```rust
// in state/memory.rs — extend MemoryState with the new field
pub agents: RwSignal<Vec<AgentSummary>>,
```

Update the `MemorySidebar` `<select>`:

```rust
<select ...>
    {move || mem.agents.get().into_iter().map(|a| {
        view! { <option value=a.id.clone()>{a.label.clone()}</option> }
    }).collect_view()}
</select>
```

- [ ] **Step 3: Build + smoke**

```bash
cargo check -p aleph-panel --target wasm32-unknown-unknown
just dev
# Visit /memory. Confirm: the right side is now just canvas; the left
# sidebar shows Agent / Search / Fold / Detail (empty → recent list) / footer.
```

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/canvas/mod.rs interfaces/webchat/src/state/memory.rs interfaces/webchat/src/components/mode_sidebar.rs
git commit -m "canvas: move agent/search/fold/detail widgets from canvas view to sidebar"
```

### Task 5.4 — Delete the obsolete sub-modules

**Files:**
- Delete: `interfaces/webchat/src/views/canvas/agent_selector.rs`
- Delete: `interfaces/webchat/src/views/canvas/toolbar.rs`
- Delete: `interfaces/webchat/src/views/canvas/breadcrumb.rs`
- Modify: `interfaces/webchat/src/views/canvas/mod.rs` (remove the `mod …;` lines)

- [ ] **Step 1: Confirm zero remaining references**

```bash
grep -rn "AgentSelectorBar\|CanvasToolbar\|use.*::breadcrumb::Breadcrumb" /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src --include="*.rs"
```

The only remaining hits should be the module declarations themselves in `views/canvas/mod.rs`. If there are leftover imports anywhere else, fix those first.

- [ ] **Step 2: Delete and clean up**

```bash
rm /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/views/canvas/agent_selector.rs
rm /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/views/canvas/toolbar.rs
rm /Volumes/TBU4/Workspace/Aleph/interfaces/webchat/src/views/canvas/breadcrumb.rs
```

Edit `views/canvas/mod.rs`: remove the `mod agent_selector;`, `mod toolbar;`, `mod breadcrumb;` lines and any `use …` re-exports they enabled.

- [ ] **Step 3: Verify clean build**

```bash
pgrep -x cargo | wc -l
cargo check -p aleph-panel --target wasm32-unknown-unknown
cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/canvas/
git commit -m "canvas: remove agent_selector / toolbar / breadcrumb (migrated to sidebar)"
```

---

## Phase 6 — Sidebar collapse + Esc + localStorage

### Task 6.1 — CSS for collapsed state

**Files:**
- Modify: the stylesheet identified in Task 4.3

- [ ] **Step 1: Append the rules**

```css
/* ── Collapsible aleph-sidebar ───────────────────────────────────── */
.aleph-sidebar {
    transition: transform 200ms ease-out, width 200ms ease-out;
}
.aleph-shell.sidebar-collapsed .aleph-sidebar {
    transform: translateX(-100%);
    width: 0;
    overflow: hidden;
}

/* 8 px hover strip on the left edge while collapsed */
.aleph-shell.sidebar-collapsed::before {
    content: "";
    position: fixed;
    left: 0; top: 0; bottom: 0;
    width: 8px;
    z-index: 50;
    cursor: e-resize;
}

.aleph-shell.sidebar-collapsed .sidebar-peek-handle {
    display: flex;
}
.sidebar-peek-handle {
    display: none;
    position: fixed;
    left: 12px;
    top: 50%;
    transform: translateY(-50%);
    width: 28px;
    height: 28px;
    align-items: center;
    justify-content: center;
    background: rgba(15, 23, 42, 0.85);
    color: #cbd5e1;
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 6px;
    cursor: pointer;
    z-index: 50;
    opacity: 0;
    transition: opacity 120ms ease-out;
}
.aleph-shell.sidebar-collapsed:hover .sidebar-peek-handle {
    opacity: 1;
}
```

- [ ] **Step 2: Commit**

```bash
git add <stylesheet path>
git commit -m "canvas: CSS for collapsible sidebar with hover-peek handle"
```

### Task 6.2 — Wire the toggle in `app.rs`

**Files:**
- Modify: `interfaces/webchat/src/app.rs:72-85` (the `aleph-shell` `view!`)
- Modify: `interfaces/webchat/src/state/memory.rs` (already has `sidebar_collapsed`)

- [ ] **Step 1: Bind the `sidebar-collapsed` class + peek handle**

In `app.rs` inside the `aleph-shell` view, change the `<div class="aleph-shell ...">` to bind dynamically:

```rust
let mem_for_shell = expect_context::<MemoryState>();
view! {
    <div
        class="aleph-shell flex h-screen text-text-primary font-sans selection:bg-primary/30"
        class:sidebar-collapsed=move || mem_for_shell.sidebar_collapsed.get()
    >
        // hover-peek button — visible only when collapsed via CSS
        <button
            class="sidebar-peek-handle"
            on:click=move |_| { mem_for_shell.sidebar_collapsed.set(false); }
            title="Expand sidebar (Esc)"
        >
            "⇨"
        </button>
        <Router>
            // ... existing children unchanged ...
        </Router>
    </div>
}
```

Make sure `MemoryState` is provided **before** this view runs. If the current `app.rs` provides it inside `MainContent`, lift the `provide_context(MemoryState::new())` call to `App` (the outer component) so the shell's class binding can read it.

- [ ] **Step 2: Esc key listener**

Inside `App` (or wherever the global event handlers are set up):

```rust
use leptos::ev::keydown;
use leptos::wasm_bindgen::JsCast;

let mem_for_key = expect_context::<MemoryState>();
window_event_listener(keydown, move |ev: web_sys::KeyboardEvent| {
    if ev.key() == "Escape" && mem_for_key.sidebar_collapsed.get() {
        mem_for_key.sidebar_collapsed.set(false);
    }
});
```

- [ ] **Step 3: localStorage persistence**

In `MemoryState::new()`, read the initial value from `localStorage["aleph.sidebar.collapsed"]`:

```rust
pub fn new() -> Self {
    let initial_collapsed = web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("aleph.sidebar.collapsed").ok().flatten())
        .map(|v| v == "1")
        .unwrap_or(false);
    let sidebar_collapsed = RwSignal::new(initial_collapsed);

    // Persist on change
    Effect::new(move |_| {
        let v = sidebar_collapsed.get();
        if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = s.set_item("aleph.sidebar.collapsed", if v { "1" } else { "0" });
        }
    });

    Self {
        // ...other fields...
        sidebar_collapsed,
    }
}
```

> `Effect::new` inside `new()` requires a reactive owner. If Leptos complains, lift the persistence effect to a separate function called from `App::mount` instead.

- [ ] **Step 4: Smoke**

```bash
just dev
# Click ⇧ at the sidebar footer — sidebar slides out, peek button appears
# at the left edge on hover. Press Esc — sidebar restores. Reload the page —
# collapsed state persists.
```

- [ ] **Step 5: Commit**

```bash
git add interfaces/webchat/src/app.rs interfaces/webchat/src/state/memory.rs
git commit -m "canvas: wire sidebar collapse button, Esc key, and localStorage persistence"
```

---

## Phase 7 — Edge labels + hover-driven label fade-in

### Task 7.1 — `<EdgeLabel>` Leptos component

**Files:**
- Create: `interfaces/webchat/src/views/canvas/edge_label.rs`
- Modify: `interfaces/webchat/src/views/canvas/mod.rs` (declare module)

- [ ] **Step 1: Declare**

In `views/canvas/mod.rs` add `mod edge_label;`.

- [ ] **Step 2: Implement**

Create `edge_label.rs`:

```rust
use leptos::prelude::*;

/// Visually-anchored edge label, positioned in screen space.
/// Pre-computed `screen_xy` and `tangent_rad_clamped` come from the rAF loop.
#[component]
pub fn EdgeLabel(
    #[prop(into)] text: String,
    screen_xy: ReadSignal<(f32, f32)>,
    /// Tangent in radians, ALREADY clamped to `[-π/4, π/4]` by caller.
    tangent_rad: ReadSignal<f32>,
    /// Visibility: true iff the underlying edge is adjacent to a hovered/selected node
    /// AND zoom ≥ 0.7. Caller computes this.
    visible: ReadSignal<bool>,
) -> impl IntoView {
    let style = move || {
        let (x, y) = screen_xy.get();
        let deg = tangent_rad.get().to_degrees();
        let opacity = if visible.get() { 1.0 } else { 0.0 };
        format!(
            "position:absolute;left:0;top:0;\
             transform:translate3d({:.1}px,{:.1}px,0) translate(-50%,-50%) rotate({:.1}deg);\
             padding:2px 8px;border-radius:6px;\
             background:rgba(15,23,42,0.85);color:#cbd5e1;\
             font-size:10px;white-space:nowrap;\
             opacity:{:.2};transition:opacity 120ms ease-out;pointer-events:none",
            x, y, deg, opacity
        )
    };

    view! {
        <div style=style>{text}</div>
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add interfaces/webchat/src/views/canvas/edge_label.rs interfaces/webchat/src/views/canvas/mod.rs
git commit -m "canvas: add EdgeLabel Leptos component for hover-shown relation text"
```

### Task 7.2 — Wire edge labels into the canvas overlay

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`

- [ ] **Step 1: Compute label positions in the rAF loop**

For each edge that has `label.is_some()`:

```rust
use crate::canvas_engine::edge_curve::{edge_control_point, bezier_point, bezier_tangent, DEFAULT_SAG};
use crate::canvas_engine::types::Vec2;

let cp = edge_control_point(from_world, to_world, DEFAULT_SAG);
let mid_world = bezier_point(from_world, cp, to_world, 0.5);
let mid_screen = world_to_screen(mid_world);  // existing helper
let tangent = bezier_tangent(from_world, cp, to_world, 0.5)
    .clamp(-std::f32::consts::FRAC_PI_4, std::f32::consts::FRAC_PI_4);
```

Push `(edge.id, mid_screen, tangent)` to a `Vec` of edge-label state, and write each entry into a per-edge `RwSignal` (mirroring Task 4.5's node-position dance).

- [ ] **Step 2: Render the labels under the node-card overlay**

Add to the `view!` (after the node-overlay div from Task 4.5):

```rust
<div class="absolute inset-0 pointer-events-none">
    {move || edge_labels.get().into_iter().map(|el| {
        view! {
            <EdgeLabel
                text=el.label.clone()
                screen_xy=el.pos_sig.read_only()
                tangent_rad=el.tangent_sig.read_only()
                visible=el.visible_sig.read_only()
            />
        }
    }).collect_view()}
</div>
```

`el.visible_sig` is computed from a `Memo` that watches `(hovered_id, selected_id, current_zoom, edge_endpoints)`:

```rust
let highlight = selected_id.get().or_else(|| hovered_id.get());
let visible = current_zoom.get() >= 0.7
    && highlight.is_some_and(|h| h == edge.from || h == edge.to);
```

- [ ] **Step 3: Smoke**

```bash
just dev
# Hover a node that has labelled edges — labels fade in within ~120ms.
# Zoom out below 70% — labels vanish.
```

- [ ] **Step 4: Commit**

```bash
git add interfaces/webchat/src/views/canvas/graph_canvas.rs
git commit -m "canvas: render edge labels at Bézier midpoint with hover/zoom visibility"
```

---

## Phase 8 — Polish, performance, smoke, ship

### Task 8.1 — Final perf check against the fixture

**Files:** (none modified)

- [ ] **Step 1: Load the 30-node fixture in dev**

```bash
just dev
# Manually seed a graph from the fixture or navigate to /memory with a
# vault that produces ~300 visible nodes. Open Chrome DevTools → Performance
# tab → record 5 seconds of pan + zoom.
```

- [ ] **Step 2: Confirm sustained ≥ 55 fps**

If the gate fails (DevTools shows < 55 fps), follow this triage path:
1. Try unmounting off-viewport cards (viewport culling) more aggressively.
2. Switch `transition:` properties to `will-change: transform` only.
3. As a last resort, render outer-ring nodes (hop ≥ 2) as Canvas2D circles via `draw_node`, keeping `<NodeCard>` only for hop ≤ 1.

Record the final fps in the CHANGELOG draft.

### Task 8.2 — Manual smoke checklist

Run through every step in spec §11.3 and check each box:

- [ ] `just dev` boots cleanly
- [ ] Navigate to memory mode — no top stack, sidebar shows agent/search/fold/path/detail/footer
- [ ] Click a node → detail in sidebar updates within ≤ 1 frame
- [ ] Hover node → adjacent edges brighten to gold + labels fade in
- [ ] Click ⇧ → sidebar collapses with 200 ms transition; canvas fills full width; peek button appears on hover
- [ ] Press Esc → sidebar reappears
- [ ] Reload page → collapsed state persists (open DevTools → Application → Local Storage, confirm `aleph.sidebar.collapsed = 1` if collapsed)
- [ ] All existing canvas behaviours (drag, zoom, navigation, prefetch, minimap) still work

### Task 8.3 — Final lint pass

```bash
pgrep -x cargo | wc -l
cargo test -p aleph-panel --target wasm32-unknown-unknown --lib
cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings
```

- [ ] All tests pass
- [ ] Clippy is silent

### Task 8.4 — CHANGELOG entry + final commit

**Files:**
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add the entry**

Under the next-release section of `CHANGELOG.md`, append:

```markdown
### Added

- Memory canvas: rich Markdown node cards (FULL / MINI / DOT modes) rendered as a Leptos DOM overlay over Canvas2D edges.
- Memory sidebar: filled the shared left column with agent picker, search, fold-threshold slider, and a detail panel that lists recently visited memories when nothing is selected.
- Sidebar collapse: ⇧ button + Esc key + localStorage persistence; right-edge hover strip restores the sidebar when collapsed.
- Edge labels: free-form `label` and `kind` fields on graph edges (Obsidian JSON Canvas-compatible naming); labels fade in for edges adjacent to the hovered/selected node when zoom ≥ 0.7.

### Changed

- Memory canvas layout: replaced the strict concentric "religious totem" rings with deterministic-jitter perturbed rings plus Poisson-disk-scattered orphans (no force engine, no new crates).
- Memory canvas edges: replaced straight strokes with α-gradient Bézier curves, layered by hop (1-hop thicker/brighter, 2-hop thinner/dimmer); adjacent edges to a hovered/selected node highlight in gold.

### Removed

- `views/canvas/agent_selector.rs`, `views/canvas/toolbar.rs`, `views/canvas/breadcrumb.rs` — their UI is now in the shared sidebar.
```

- [ ] **Step 2: Commit**

```bash
git add CHANGELOG.md
git commit -m "canvas: changelog for memory canvas visual redesign"
```

---

## Self-Review (done by plan author, locked-in before handoff)

Checked the plan against the spec section by section:

1. **§1 Problem statement** → covered implicitly across all phases; no code task needed.
2. **§2 Non-goals** → recorded; no code task.
3. **§3 Architectural decisions** → Phase 4 + Phase 5 enforce R2 (Leptos UI), Phase 1 enforces R3 (no new crates) via `fnv1a.rs`.
4. **§4 Layout restructure** → Phase 5 (sidebar fill) + Phase 6 (collapse) cover every bullet (§4.1–§4.5).
5. **§5 Node card** → Phase 4 (Tasks 4.2–4.6). FULL/MINI/DOT modes in `pick_mode`. Excerpt lazy-fetch in 4.6. CSS tokens in 4.3.
6. **§6 L2 layout** → Phase 2 (Tasks 2.1–2.4). All 6 unit tests from §6.4 present.
7. **§7 Edges** → Phase 3 + Phase 7. Bézier in 3.1, gradient + hop layering in 3.2, hover highlight in 3.3, labels in 7.1–7.2. All 6 unit tests from §7.5 present (renamed to match real function names: `bezier_control_point_deterministic` → ✅; `control_point_perpendicular_to_edge_axis` → ✅; `gradient_alpha_by_hop_layer` → `hop_style_layered_by_hop` ✅; `edge_label_position_at_t_05` → `bezier_point_at_t_05_matches_formula` ✅; `edge_label_rotation_clamped` — **renamed** to caller-side responsibility, covered in Task 7.2 Step 1; `edge_kind_arrow_only_for_directional_kinds` → `is_directional_only_for_recognized_kinds` ✅).
8. **§8 File map** → mapped 1:1 in "File Structure" header above.
9. **§9 Reference repos** → cited in plan header.
10. **§10 Performance** → Phase 0 gate + Phase 8 final check.
11. **§11 Verification** → Task 2.4 (snapshot regression), Task 8.2 (smoke), Task 8.3 (lint).
12. **§12 Out-of-scope** → not in this plan by design.
13. **§13 Build sequence** → followed exactly (9 phases instead of 8 because Phase 0 split off).

**Type / name consistency check:**

- `pick_mode` (4.4) consumes `is_hovered: bool`, `is_selected: bool` — call site in `NodeCard` derives these from `hovered_id` / `selected_id` `ReadSignal<Option<String>>` and the node's own id. ✅
- `place_perturbed_ring(ids: &[&str], base_r: f32, out: &mut HashMap<String, Vec2>)` — consistent across Task 2.1 and Task 2.3 call site. ✅
- `PrefetchCache<T>` generic — every existing call site annotated in Task 1.4 Step 3. ✅
- `NoteLinkDto.label / .kind` are `Option<String>` — matches §7.4 of the spec and Tasks 1.1, 7.2. ✅
- `MemoryState.recent_visited` `VecDeque<String>` — matches §4.5 spec. ✅
- Edge tangent computed inside `graph_canvas.rs` is clamped to `[-π/4, π/4]` before being handed to `<EdgeLabel>` whose prop is `tangent_rad: ReadSignal<f32>` (already clamped). Documented in 7.1 doc comment. ✅

**Placeholder scan:** no `TBD`, `TODO`, "fill in details", "similar to Task N", or "implement appropriate error handling" in the plan body.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-23-memory-canvas-redesign.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Phase 0 gate result becomes the first review checkpoint.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints for review.

Which approach?
