# Memory Canvas — Hover / Perf / Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the memory knowledge-graph canvas hover stable & readable, idle the render loop to zero CPU when hidden, replace per-frame layout reads with a ResizeObserver, and delete the dead card-click handler.

**Architecture:** Single hover authority on the canvas with two-level hysteresis (small enter circle, large retention box covering the Full card). rAF parks itself when hidden and is re-kicked by the existing IntersectionObserver. A ResizeObserver feeds size changes through a `Cell` instead of `getBoundingClientRect` every frame. Dead `on_click` plumbing removed; a CSS fade softens mode swaps.

**Tech Stack:** Rust + Leptos 0.8 (wasm32), web-sys (Canvas2D, IntersectionObserver, ResizeObserver), Tailwind CSS.

**Session constraint:** Per user mandate, do **NOT** run `cargo check` / tests this session. Unit tests are authored and committed for later (CI / next session). Each "run" step records the expected result but is explicitly deferred. All work happens in worktree `/Volumes/TBU4/Workspace/Aleph-wt-canvas` (branch `feat/canvas-hover-perf`); commit after each task.

**Spec:** `docs/superpowers/specs/2026-06-07-canvas-hover-perf-design.md`

---

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `interfaces/webchat/Cargo.toml` | web-sys feature set | add ResizeObserver features |
| `interfaces/webchat/src/canvas_engine/viewport.rs` | pure viewport math + hit/hover tests | add `hover_retains` + unit tests |
| `interfaces/webchat/src/views/canvas/graph_canvas.rs` | rAF loop, pointer handlers, overlay | hysteresis wiring, anim-freeze, rAF park, ResizeObserver, remove dead click |
| `interfaces/webchat/src/views/canvas/node_card.rs` | overlay card component | remove `on_click` prop + handlers |
| `interfaces/webchat/styles/tailwind.css` | canvas card styling | mode-swap fade-in keyframe |

All edits are inside the `webchat` panel crate. Working directory for every command: `/Volumes/TBU4/Workspace/Aleph-wt-canvas`.

---

## Task 1: Enable ResizeObserver in web-sys

**Files:**
- Modify: `interfaces/webchat/Cargo.toml`

- [ ] **Step 1: Add the ResizeObserver feature trio**

In `interfaces/webchat/Cargo.toml`, find the `web-sys` features array (the line beginning `"IntersectionObserver", "IntersectionObserverEntry",`) and insert directly after it:

```toml
    # graph_canvas resize without per-frame getBoundingClientRect layout reads
    "ResizeObserver", "ResizeObserverEntry", "DomRectReadOnly",
```

- [ ] **Step 2: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-canvas
git add interfaces/webchat/Cargo.toml
git commit -m "webchat: enable web-sys ResizeObserver features"
```

---

## Task 2: Hover hysteresis in the viewport (`hover_retains`)

**Files:**
- Modify: `interfaces/webchat/src/canvas_engine/viewport.rs`
- Test: `interfaces/webchat/src/canvas_engine/viewport.rs` (in-file `#[cfg(test)]`)

- [ ] **Step 1: Write the failing tests**

Add these tests to the existing `#[cfg(test)] mod tests` block in `viewport.rs` (alongside the `hit_test_*` tests):

```rust
    #[test]
    fn hover_retains_inside_full_card_box() {
        // scale 1.0, node at world origin → screen center (400,300).
        let vp = Viewport::new(800.0, 600.0);
        // A point over the card body (down-right of the node) but well outside
        // the bare node circle still retains hover.
        assert!(vp.hover_retains(Vec2::new(500.0, 350.0), Vec2::zero(), 10.0));
    }

    #[test]
    fn hover_retains_false_outside_box() {
        let vp = Viewport::new(800.0, 600.0);
        // 200 px right of center exceeds RETAIN_HALF_W (150).
        assert!(!vp.hover_retains(Vec2::new(600.0, 300.0), Vec2::zero(), 10.0));
        // 140 px below center exceeds RETAIN_DOWN (130).
        assert!(!vp.hover_retains(Vec2::new(400.0, 440.0), Vec2::zero(), 10.0));
    }

    #[test]
    fn hover_retains_dot_mode_uses_circle() {
        // Below the Dot threshold (scale < 0.5) there is no enlarged card, so
        // retention degrades to the forgiving circle (radius*scale + tol).
        let mut vp = Viewport::new(800.0, 600.0);
        vp.scale = 0.4; // offset stays (400,300); world origin → screen (400,300)
        // radius 10 → 10*0.4 + 6 = 10 px screen tolerance.
        assert!(vp.hover_retains(Vec2::new(405.0, 300.0), Vec2::zero(), 10.0));
        assert!(!vp.hover_retains(Vec2::new(420.0, 300.0), Vec2::zero(), 10.0));
    }
```

- [ ] **Step 2: Run tests to verify they fail** *(DEFERRED — do not run cargo this session)*

Run (later): `cargo test -p aleph-panel --lib canvas_engine::viewport`
Expected: FAIL — `no method named hover_retains`.

- [ ] **Step 3: Add the retention constants and method**

In `viewport.rs`, directly below the `const HIT_TOLERANCE_PX: f64 = 6.0;` declaration (after its doc comment, before `pub struct Viewport`), add:

```rust
/// Screen-space half-extents of the hover-retention box around a held node's
/// screen center, sized to cover the Full card footprint. The card is 280 px
/// wide and positioned via `translate3d(x-140, y-60)`, with the excerpt
/// extending downward — hence the asymmetric vertical extents. Used for hover
/// *hysteresis*: entry uses the bare node circle, retention uses this larger
/// region so a held node keeps hover while the pointer rests over its card.
const RETAIN_HALF_W: f64 = 150.0;
const RETAIN_UP: f64 = 70.0;
const RETAIN_DOWN: f64 = 130.0;
```

Then inside `impl Viewport`, directly after the `hit_test` method (after its closing `}`), add:

```rust
    /// Hover hysteresis: returns true if `screen_point` still falls within the
    /// retention region of the already-held node at `node_world`.
    ///
    /// `hit_test` (the bare circle) decides hover *entry*; this larger region
    /// decides *retention*, so a held node keeps hover while the pointer rests
    /// anywhere over its enlarged Full card — killing boundary flicker and
    /// letting the user move onto the card to read it. Below the Dot zoom
    /// threshold (`scale < 0.5`) the node renders as a dot with no enlarged
    /// card, so retention degrades to the same forgiving circle as entry.
    pub fn hover_retains(&self, screen_point: Vec2, node_world: Vec2, node_radius: f64) -> bool {
        let center = self.world_to_screen(node_world);
        if self.scale < 0.5 {
            return screen_point.distance_to(&center) <= node_radius * self.scale + HIT_TOLERANCE_PX;
        }
        let dx = screen_point.x - center.x;
        let dy = screen_point.y - center.y;
        dx >= -RETAIN_HALF_W && dx <= RETAIN_HALF_W && dy >= -RETAIN_UP && dy <= RETAIN_DOWN
    }
```

- [ ] **Step 4: Run tests to verify they pass** *(DEFERRED)*

Run (later): `cargo test -p aleph-panel --lib canvas_engine::viewport`
Expected: PASS (all `hover_retains_*` + existing `hit_test_*`).

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-canvas
git add interfaces/webchat/src/canvas_engine/viewport.rs
git commit -m "canvas: add hover_retains hysteresis to viewport"
```

---

## Task 3: Wire hysteresis + animation-freeze into pointer-move

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`

- [ ] **Step 1: Pre-clone `nav` for the move handler**

In `graph_canvas.rs`, find the nav clone block (currently):

```rust
    let nav_for_md = nav.clone();
    let nav_for_mu = nav.clone();
```

Replace with:

```rust
    let nav_for_md = nav.clone();
    let nav_for_mm = nav.clone();
    let nav_for_mu = nav.clone();
```

(The `nav` value is moved into the rAF Effect below, so the move handler must capture its own clone made here.)

- [ ] **Step 2: Replace the hover branch in `on_pointermove`**

In `on_pointermove`, find the final `else` branch (the hover detection):

```rust
        } else {
            // Hover detection
            let hit = state.viewport.hit_test(screen, &state.nodes);
            let new_hovered = hit.and_then(|idx| state.nodes.get(idx).map(|n| n.id.clone()));
            if new_hovered != state.hovered_node {
                state.hovered_node = new_hovered.clone();
                drop(state);
                on_event.run(CanvasEvent::HoverNode(new_hovered));
            }
        }
```

Replace it with:

```rust
        } else {
            // Freeze hover during a retarget/focus tween: hit-test reads target
            // positions from state.nodes while the renderer draws interpolated
            // positions, so updating hover mid-animation picks the wrong node.
            if let Some(ref nav_rc) = nav_for_mm {
                if nav_rc.borrow().is_animating() {
                    return;
                }
            }

            // Hover hysteresis: keep the held node while the pointer rests
            // anywhere over its enlarged card footprint (hover_retains); only
            // re-test for a new node once the pointer leaves that region. This
            // kills the boundary flicker and lets the user move onto the card.
            let retained = match state.hovered_node.as_deref() {
                Some(hid) => state
                    .nodes
                    .iter()
                    .find(|n| n.id == hid)
                    .map(|n| state.viewport.hover_retains(screen, n.position, n.radius))
                    .unwrap_or(false),
                None => false,
            };

            if !retained {
                let hit = state.viewport.hit_test(screen, &state.nodes);
                let new_hovered = hit.and_then(|idx| state.nodes.get(idx).map(|n| n.id.clone()));
                if new_hovered != state.hovered_node {
                    state.hovered_node = new_hovered.clone();
                    drop(state);
                    on_event.run(CanvasEvent::HoverNode(new_hovered));
                }
            }
        }
```

- [ ] **Step 3: Verify (compile check DEFERRED)**

Expected when built: no borrow errors — `state.hovered_node.as_deref()` and `state.nodes.iter()` are both shared borrows of disjoint fields; the `retained` bool is computed before any mutable borrow.

- [ ] **Step 4: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-canvas
git add interfaces/webchat/src/views/canvas/graph_canvas.rs
git commit -m "canvas: hover hysteresis + freeze hover during tween"
```

---

## Task 4: Park the rAF loop when hidden (zero CPU), resume via observer

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`

- [ ] **Step 1: Declare the `parked` flag inside the Effect**

In the render Effect, immediately after `let canvas: web_sys::HtmlCanvasElement = canvas_el;`, add:

```rust
        // Park flag: set true when the rAF loop stops itself because the canvas
        // is hidden. The IntersectionObserver callback clears it and re-kicks a
        // frame when the canvas becomes visible again — so a hidden Memory tab
        // costs zero CPU instead of rescheduling rAF ~60×/s just to re-check.
        let parked: std::rc::Rc<std::cell::Cell<bool>> = std::rc::Rc::new(std::cell::Cell::new(false));
```

- [ ] **Step 2: Make the IntersectionObserver callback resume the loop**

Find the observer callback setup:

```rust
        let is_visible_obs = is_visible_for_effect.clone();
        let observer_cb: Closure<dyn FnMut(js_sys::Array)> =
            Closure::new(move |entries: js_sys::Array| {
                if let Ok(entry_val) = entries
                    .get(0)
                    .dyn_into::<web_sys::IntersectionObserverEntry>()
                {
                    is_visible_obs.set(entry_val.is_intersecting());
                }
            });
```

Replace with:

```rust
        let is_visible_obs = is_visible_for_effect.clone();
        let parked_obs = parked.clone();
        let raf_c_for_obs = raf_c.clone();
        let raf_h_for_obs = raf_h.clone();
        let observer_cb: Closure<dyn FnMut(js_sys::Array)> =
            Closure::new(move |entries: js_sys::Array| {
                if let Ok(entry_val) = entries
                    .get(0)
                    .dyn_into::<web_sys::IntersectionObserverEntry>()
                {
                    let vis = entry_val.is_intersecting();
                    is_visible_obs.set(vis);
                    // Resume a parked loop on becoming visible again.
                    if vis && parked_obs.get() {
                        parked_obs.set(false);
                        if let Some(window) = web_sys::window() {
                            if let Some(closure) = raf_c_for_obs.borrow().as_ref() {
                                let id = window
                                    .request_animation_frame(closure.as_ref().unchecked_ref())
                                    .unwrap_or(0);
                                *raf_h_for_obs.borrow_mut() = Some(id);
                            }
                        }
                    }
                }
            });
```

- [ ] **Step 3: Capture `parked` into the rAF closure**

Find the inner-clone block just before the rAF `Closure::new` (near `let is_visible_for_raf = is_visible.clone();`) and add a clone beside it:

```rust
        let is_visible_for_raf = is_visible.clone();
        let parked_for_raf = parked.clone();
```

- [ ] **Step 4: Replace the visibility-pause branch to stop rescheduling**

Inside the rAF closure, find the current hidden branch:

```rust
            if !is_visible_for_raf.get() {
                if let Some(window) = web_sys::window() {
                    if let Some(closure) = raf_c_inner.borrow().as_ref() {
                        let id = window
                            .request_animation_frame(closure.as_ref().unchecked_ref())
                            .unwrap_or(0);
                        *raf_h_inner.borrow_mut() = Some(id);
                    }
                }
                return;
            }
```

Replace with:

```rust
            if !is_visible_for_raf.get() {
                // Park: stop the rAF chain entirely (zero CPU while hidden).
                // The IntersectionObserver callback re-kicks one frame when the
                // canvas becomes visible again.
                parked_for_raf.set(true);
                return;
            }
```

- [ ] **Step 5: Verify (compile check DEFERRED)**

Expected when built: `raf_c`/`raf_h` are still in scope at the observer setup (cloned before being moved into the rAF closure); single-threaded wasm so `Rc<Cell<bool>>` is sound.

- [ ] **Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-canvas
git add interfaces/webchat/src/views/canvas/graph_canvas.rs
git commit -m "canvas: park rAF when hidden, resume via IntersectionObserver"
```

---

## Task 5: ResizeObserver instead of per-frame getBoundingClientRect

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`

- [ ] **Step 1: Set up the ResizeObserver in the Effect**

In the render Effect, after the IntersectionObserver block (after `observer_cb.forget();`) and before the `// Initial canvas size` block, add:

```rust
        // Parent-size channel: the ResizeObserver writes the latest content-box
        // size here; the rAF loop reads it (cheap) instead of calling
        // get_bounding_client_rect every frame (which forces synchronous
        // layout — the same reason visibility uses IntersectionObserver above).
        let pending_size: std::rc::Rc<std::cell::Cell<Option<(f64, f64)>>> =
            std::rc::Rc::new(std::cell::Cell::new(None));
        let pending_size_obs = pending_size.clone();
        let resize_cb: Closure<dyn FnMut(js_sys::Array)> =
            Closure::new(move |entries: js_sys::Array| {
                if let Ok(entry) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() {
                    let rect = entry.content_rect();
                    let w = rect.width().max(1.0);
                    let h = rect.height().max(1.0);
                    pending_size_obs.set(Some((w, h)));
                }
            });
        if let Ok(observer) = web_sys::ResizeObserver::new(resize_cb.as_ref().unchecked_ref()) {
            if let Some(parent) = canvas.parent_element() {
                observer.observe(&parent);
            }
        }
        // Leak the resize callback for the panel's lifetime — same rationale as
        // the IntersectionObserver and rAF closures (parent never unmounts us).
        resize_cb.forget();
```

- [ ] **Step 2: Capture `pending_size` into the rAF closure**

Beside the `let parked_for_raf = parked.clone();` line added in Task 4 Step 3, add:

```rust
        let pending_size_for_raf = pending_size.clone();
```

- [ ] **Step 3: Replace the per-frame resize block**

Inside the rAF closure, find the dynamic-resize block:

```rust
            // Dynamic canvas resize: check parent size each frame
            if let Some(parent) = canvas_for_resize.parent_element() {
                let rect = parent.get_bounding_client_rect();
                let pw = rect.width();
                let ph = rect.height();
                if pw > 1.0 && ph > 1.0 {
                    let cur_w = canvas_for_resize.width() as f64 / dpr;
                    let cur_h = canvas_for_resize.height() as f64 / dpr;
                    if (pw - cur_w).abs() > 1.0 || (ph - cur_h).abs() > 1.0 {
                        canvas_for_resize.set_width((pw * dpr) as u32);
                        canvas_for_resize.set_height((ph * dpr) as u32);
                        let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
                        state.viewport.width = pw;
                        state.viewport.height = ph;
                        state.viewport.offset.x = pw / 2.0;
                        state.viewport.offset.y = ph / 2.0;
                        // Refit content to the new canvas size. Only when nodes are loaded
                        // (otherwise nodes is empty and fit_to_content early-returns).
                        // Reborrow through `&mut *state` so the borrow checker sees disjoint
                        // field borrows on GraphState rather than two simultaneous borrows of
                        // the RefMut wrapper.
                        if !state.nodes.is_empty() {
                            let state = &mut *state;
                            state.viewport.fit_to_content(&state.nodes, 0.10);
                        }
                    }
                }
            }
```

Replace it with:

```rust
            // Apply a pending resize from the ResizeObserver, if any. take()
            // consumes the cell so we only resize when a new size actually
            // arrived — no per-frame getBoundingClientRect layout read.
            if let Some((pw, ph)) = pending_size_for_raf.take() {
                if pw > 1.0 && ph > 1.0 {
                    let cur_w = canvas_for_resize.width() as f64 / dpr;
                    let cur_h = canvas_for_resize.height() as f64 / dpr;
                    if (pw - cur_w).abs() > 1.0 || (ph - cur_h).abs() > 1.0 {
                        canvas_for_resize.set_width((pw * dpr) as u32);
                        canvas_for_resize.set_height((ph * dpr) as u32);
                        let _ = ctx.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0);
                        state.viewport.width = pw;
                        state.viewport.height = ph;
                        state.viewport.offset.x = pw / 2.0;
                        state.viewport.offset.y = ph / 2.0;
                        // Refit content to the new canvas size. Only when nodes are loaded
                        // (otherwise nodes is empty and fit_to_content early-returns).
                        // Reborrow through `&mut *state` so the borrow checker sees disjoint
                        // field borrows on GraphState rather than two simultaneous borrows of
                        // the RefMut wrapper.
                        if !state.nodes.is_empty() {
                            let state = &mut *state;
                            state.viewport.fit_to_content(&state.nodes, 0.10);
                        }
                    }
                }
            }
```

- [ ] **Step 4: Verify (compile check DEFERRED)**

Expected when built: `Cell::<Option<(f64,f64)>>::take()` resolves (Option is Default); `content_rect()` returns `DomRectReadOnly` (feature enabled in Task 1); `canvas_for_resize` is still used (the `.width()/.height()` calls remain).

- [ ] **Step 5: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-canvas
git add interfaces/webchat/src/views/canvas/graph_canvas.rs
git commit -m "canvas: ResizeObserver replaces per-frame layout read"
```

---

## Task 6: Remove dead `on_card_click` plumbing

**Files:**
- Modify: `interfaces/webchat/src/views/canvas/graph_canvas.rs`
- Modify: `interfaces/webchat/src/views/canvas/node_card.rs`

Context: cards inherit `pointer-events: none` from the overlay wrapper, so `on:click` never fires — clicks fall through to the canvas hit-test (`CanvasEvent::SelectNode`). The handler also only set a local signal, never navigating. It is dead code.

- [ ] **Step 1: Remove the handler construction in `graph_canvas.rs`**

In the node overlay closure (`overlay_nodes.get()...map(...)`), find:

```rust
                        let id_clone = n.id.clone();
                        // Get-or-create the per-node screen position signal.
                        let pos_sig: RwSignal<(f32, f32)> =
                            node_screen_pos.with(|m| m.get(&n.id).copied())
                                .unwrap_or_else(|| RwSignal::new((0.0_f32, 0.0_f32)));

                        let on_card_click = Callback::new(move |_id: String| {
                            selected_id_sig.set(Some(id_clone.clone()));
                        });

                        let id_lookup = n.id.clone();
```

Replace with (drop `id_clone` and `on_card_click`):

```rust
                        // Get-or-create the per-node screen position signal.
                        let pos_sig: RwSignal<(f32, f32)> =
                            node_screen_pos.with(|m| m.get(&n.id).copied())
                                .unwrap_or_else(|| RwSignal::new((0.0_f32, 0.0_f32)));

                        let id_lookup = n.id.clone();
```

- [ ] **Step 2: Remove the `on_click` prop pass on `<NodeCard>`**

In the same `view! { <NodeCard ... /> }`, delete the line:

```rust
                                on_click=on_card_click
```

(Leave every other prop — `id`, `name`, `screen_xy`, `hovered_id`, `selected_id`, etc. — unchanged.)

- [ ] **Step 3: Remove the `on_click` prop from the `NodeCard` component**

In `node_card.rs`, delete the prop declaration at the end of the argument list:

```rust
    /// Click handler — receives the node id.
    on_click: Callback<String>,
```

- [ ] **Step 4: Remove the three `on:click` handlers and their `click_id` bindings**

In the `CardMode::Full` arm, delete:

```rust
            let click_id = id.clone();
```
and
```rust
                    on:click=move |_| on_click.run(click_id.clone())
```

In the `CardMode::Mini` arm, delete:

```rust
            let click_id = id.clone();
```
and
```rust
                    on:click=move |_| on_click.run(click_id.clone())
```

In the `CardMode::Dot` arm, delete:

```rust
            let click_id = id.clone();
```
and
```rust
                    on:click=move |_| on_click.run(click_id.clone())
```

- [ ] **Step 5: Verify (compile check DEFERRED)**

Expected when built: no unused-variable / unused-import warnings — `Callback` was only used by `on_click`; it is re-exported via `leptos::prelude::*` (glob), so no explicit `use` line needs deleting. `id` is still consumed by the `data-id`/`Memo` clones in each arm.

- [ ] **Step 6: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-canvas
git add interfaces/webchat/src/views/canvas/graph_canvas.rs interfaces/webchat/src/views/canvas/node_card.rs
git commit -m "canvas: remove dead on_card_click handler"
```

---

## Task 7: Soften mode swaps with a CSS fade-in

**Files:**
- Modify: `interfaces/webchat/styles/tailwind.css`

Context: switching Dot→Mini→Full swaps the whole card element (the `match mode.get()` returns a different node), so it pops in. A mount fade-in on the card classes removes the harshness without restructuring the component.

- [ ] **Step 1: Add the keyframe + animation rule**

In `styles/tailwind.css`, immediately after the `.node-card-dot:hover { ... }` rule (the end of the node-card block, before the `/* ── Sidebar node markdown editor ── */` comment), add:

```css

/* Soften Dot↔Mini↔Full swaps: the reactive match remounts the card element,
   so a mount fade-in removes the harsh pop. Opacity-only — must not touch the
   translate3d transform that positions the card each frame. */
@keyframes node-card-in {
    from { opacity: 0; }
    to   { opacity: 1; }
}
.node-card-full,
.node-card-mini,
.node-card-dot {
    animation: node-card-in 120ms ease-out;
}
@media (prefers-reduced-motion: reduce) {
    .node-card-full,
    .node-card-mini,
    .node-card-dot { animation: none; }
}
```

- [ ] **Step 2: Commit**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-canvas
git add interfaces/webchat/styles/tailwind.css
git commit -m "canvas: fade-in card on mode swap"
```

---

## Task 8: Wrap-up

- [ ] **Step 1: Review the branch diff**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-canvas
git log --oneline main..HEAD
git diff main..HEAD --stat
```

Expected: 7 functional commits + the spec commit; files touched limited to the five in the File Structure table.

- [ ] **Step 2: Leave the worktree for the user to merge**

Per `CLAUDE.md`, do **not** `git worktree remove` in this session. Report the branch (`feat/canvas-hover-perf`) and worktree path; the user merges and runs `cargo check` / `just wasm` + binary rebuild when ready (panel changes require the WASM→rust_embed→binary refresh chain).

---

## Self-Review

**Spec coverage:**
- 模块一 hover 滞回 → Task 2 (`hover_retains` + tests) + Task 3 (wiring + anim-freeze). ✓
- 模块一 平滑过渡 → Task 7 (fade-in; simplified from the spec's wrapper approach to an opacity mount-fade — same felt result, lower risk, noted). ✓
- 模块二 rAF 空转 → Task 4. ✓
- 模块二 ResizeObserver → Task 1 (feature) + Task 5. ✓
- 模块三 死代码 → Task 6. ✓
- Out of scope (JSON Canvas, memory-events, backend) → untouched. ✓

**Placeholder scan:** No TBD/TODO-as-work; every code step shows full code. The deferred "run" steps are an explicit session constraint, not missing content.

**Type consistency:** `hover_retains(&self, screen_point: Vec2, node_world: Vec2, node_radius: f64) -> bool` defined in Task 2, called identically in Task 3. `parked: Rc<Cell<bool>>` and `pending_size: Rc<Cell<Option<(f64,f64)>>>` declared in Tasks 4/5 and captured under the same names. `on_click`/`on_card_click` removed consistently across both files in Task 6.
