# Upstream deliverable 1 — `DOMSnapshot.captureSnapshot` should report the real layout

> **This file is for a human to submit.** Aleph does not open pull requests against other people's
> repositories, and this round did not.
>
> It is a PR description plus a patch *sketch* against the obscura clone at
> `/Volumes/TBU4/Github/obscura`. **Every line anchor below was re-measured on 2026-09-20 against
> that clone's working tree, not carried over from the survey** — and that re-measurement is the
> reason this paragraph exists:
>
> - `obscura-source-survey.md` §0 recorded the clone at
>   `72c84adcc6ec3ea4a7144adb4e45d4d3038ebcda` and stated, correctly for the day it was written,
>   that the clone **had never been fetched**.
> - It has since. `.git/FETCH_HEAD` and `.git/ORIG_HEAD` are dated 2026-09-12; `ORIG_HEAD` is
>   `72c84ad` and `refs/heads/main` is now
>   **`eec047a188cc75b7a1a257397ad84493ee59c091`**, fetched from
>   `https://github.com/h4ckf0r0day/obscura`.
> - So the survey's anchors are the *old* ones and several have drifted. The table below records
>   both, because "the survey said `:986`" and "the file says `:1051`" are two different facts and
>   only one of them helps a reviewer open the file.
>
> **Nothing here has been compiled** — this branch does not build obscura. Treat the code below as
> the shape of the change, not as a diff to apply blind. Re-read the two files before submitting;
> upstream may have moved again since 2026-09-12.

## Title

`cdp: DOMSnapshot.captureSnapshot should report real layout in render builds`

## Problem

`DOMSnapshot.captureSnapshot` returns synthesized geometry, and says so itself
(`crates/obscura-cdp/src/domains/domsnapshot.rs:1-15`):

> "Obscura has no layout/paint engine, so there is no real geometry to report. We synthesize it:
> every node gets a distinct, on-screen, non-icon-sized box (a simple vertical stack) plus plausible
> computed styles (visible, opaque, pointer cursor on interactive tags) … Clicking still falls back
> to JS `.click()` since the coordinates are synthetic."

The implementation (`domsnapshot.rs:231-236`) is a vertical stack:

```rust
// Synthetic geometry: a vertical stack, full-width, 18px tall. Distinct
// and non-icon-sized so visibility/size heuristics include the element;
// the coordinates are not real (no layout engine).
let y = (i as f64) * 18.0;
bounds.push(json!([0.0, y, 1280.0, 18.0]));
client_rects.push(json!([0.0, y, 1280.0, 18.0]));
```

The computed styles beside it (`domsnapshot.rs:215-226`) are **eight literals and two tag-derived
values**:

```rust
let style_vals = [
    display,             // "none" for head/meta/title/script/style/link/noscript/base, else "block"
    "visible", "1", "visible", "visible", "visible",
    cursor,              // "pointer" for a|button|input|select|textarea|summary|details|option|label
    "auto", "static", "rgba(0, 0, 0, 0)",
];
```

⚠️ Stated that precisely on purpose. It would be easier to write "a constant style vector claiming
`visibility:visible, opacity:1, position:static` for **every** node", and an earlier draft of this
file did — but two of the ten entries *are* derived from the tag, and a reviewer who opens the file
and finds that discrepancy has been handed a reason to discount the rest of the report. The claim
that actually matters survives the precision: `display` is derived from a **tag allow-list**, not
from a cascade, so `<div style="display:none">` reports `block`; and `visibility` / `opacity` /
`position` / the background are literals for every node in the document.

That premise — "there is no real geometry to report" — is no longer true for a build with the
`render` feature. `obscura-render` computes a real `DomLayout`, and the screenshot path already
paints from it. But `rg 'render|layout_dom|DomLayout|PreparedRender'
crates/obscura-cdp/src/domains/domsnapshot.rs` finds exactly **one** hit, and it is the word
"render" inside a comment (`:207`, "Tags that never render box content"). The CDP domain does not
consult the render layer even when one exists.

The consequence for a CDP client is worse than a missing feature: the response is
**indistinguishable from a measurement**. Every consumer that reads `layout.bounds` (browser-use
does; so does anything that ports Chrome's snapshot path) receives a stack of 1280×18 rectangles
with correct-looking shape and no error to notice. A client cannot tell "this build has no layout
engine" from "this page really is a vertical stack of 18px rows".

The same shape exists in `DOM.getBoxModel` and `DOM.getContentQuads`. Both evaluate
`getBoundingClientRect()` in the page and, whenever that round trip returns anything unparseable,
fall back to the constant quad `[8,8,108,8,108,28,8,28]` — **as a successful response**:

| method | fallback sites | what it returns |
|---|---|---|
| `getBoxModel` | `dom.rs:335` (array present, fewer than 10 numbers) and `dom.rs:338` (not an array at all) | that quad for all four of `content`/`padding`/`border`/`margin`, plus `width: 100, height: 20` |
| `getContentQuads` | `dom.rs:369` and `dom.rs:371` | `{"quads": [that quad]}` |

There is no `null`, no error, and no flag. A client that asked "where is this element" is told
"at (8,8), 100×20" by a build that does not know.

## Proposed change

In a `render` build, source `captureSnapshot`'s bounds and computed styles from the retained
`PreparedRender` the screenshot path already holds, so the snapshot and the pixels agree by
construction. In a non-render build, keep today's behaviour but make it legible — see "Smaller
alternative".

The accessors needed are already `pub` on `PreparedRender`, in `crates/obscura-render/src/paint.rs`:

| accessor | at `eec047a1` | the survey recorded (`72c84ad`) |
|---|---|---|
| `pub fn layout(&self) -> &crate::DomLayout` | `:1051` | `:986` |
| `pub fn content_size(&self) -> (f32, f32)` | `:1055` | `:991` |
| `pub fn viewport_fixed_nodes(&self)` | `:1059` | `:995` |
| `pub fn sticky_layout(&self)` | `:1063` | `:999` |

`DomLayout` (`crates/obscura-render/src/dom.rs:298`) carries `rects: HashMap<NodeId, Rect>` (`:299`),
`styles: HashMap<NodeId, LayoutStyle>` (`:305`) and `text_runs: HashMap<NodeId, Vec<(Rect, String)>>`
(`:331`), all `pub`, alongside `inline_fragments`, `clip_rects`, `translates` and `transforms`.

**The one missing piece is a way out of `obscura-js`.** `PreparedRender` lives in
`SharedState.prepared_render`, and every `pub fn` on the runtime that touches it yields a derived
scalar or a rasterized image — `prepared_content_size()` (`runtime.rs:1773`),
`prepared_has_active_css_animations()` (`:1541`), the `screenshot_prepared*` family
(`:1553` onward). `Page` mirrors exactly those (`crates/obscura-browser/src/page.rs:4015` onward).
The only other `PreparedRender` mentions in `runtime.rs` are raw-pointer identity checks used by the
retained-render bookkeeping (`:9909` onward), which are not an access path.

### Sketch

1. `crates/obscura-js/src/runtime.rs` — hand the retained render out, the same way `with_dom`
   already hands out the DOM (`runtime.rs:3829`, which takes a `RefCell` borrow and no V8 scope):

   ```rust
   pub fn with_prepared_render<R>(
       &self,
       f: impl FnOnce(&crate::PreparedRender) -> R,
   ) -> Option<R> {
       let state = self.state.borrow();
       state.prepared_render.as_ref().map(f)
   }
   ```

2. `crates/obscura-browser/src/page.rs` — forward it, mirroring `Page::with_dom` (`page.rs:3628`):

   ```rust
   pub fn with_prepared_render<R>(
       &self,
       f: impl FnOnce(&obscura_render::PreparedRender) -> R,
   ) -> Option<R> {
       self.js.as_ref()?.with_prepared_render(f)
   }
   ```

3. `crates/obscura-cdp/src/domains/domsnapshot.rs` — replace the synthetic stack with a lookup, and
   fall back to today's behaviour only when there is no retained render:

   ```rust
   // Before: bounds.push(json!([0.0, (i as f64) * 18.0, 1280.0, 18.0]));
   // After:
   let real = page.with_prepared_render(|pr| {
       let layout = pr.layout();
       node_ids
           .iter()
           .map(|nid| layout.rects.get(nid).map(|r| json!([r.x, r.y, r.width, r.height])))
           .collect::<Vec<_>>()
   });
   match real {
       Some(rects) => {
           for r in rects {
               // `None` == this node generates no box (display:none, detached).
               // Emit an EMPTY rect, which is what Chrome emits, rather than a
               // plausible one — being able to tell is the whole point.
               bounds.push(r.unwrap_or_else(|| json!([])));
           }
       }
       None => { /* today's synthetic stack, behind the module doc's caveat */ }
   }
   ```

   and source computed styles from `layout.styles` (`LayoutStyle` already carries display,
   visibility, opacity, position and transforms) instead of the ten-entry vector at `:215-226`.

### Smaller alternative, if that is too large for one PR

Make the fabrication **legible on the wire** instead of removing it: add a non-standard
`"obscuraSynthesizedGeometry": true` to the `captureSnapshot` result whenever the layout is
synthesized, and to `getBoxModel`/`getContentQuads` when the constant quad is returned. That fixes
nothing, but it turns a silent fabrication into a fact a client can branch on — which is the
difference between "we cannot measure this" and "we measured this".

## What this is worth downstream

Aleph currently ships an interim fetcher (`src/browser/page_state/fetch_obscura.rs`) that walks
`DOM.getDocument` and issues one `DOM.getBoxModel` per node with bounded concurrency, then
cross-checks against a `Runtime.evaluate` of the computed style and drops the geometry when the two
disagree. That is slower than one `captureSnapshot`, and it is written to be deleted: its module
doc names the condition — the runtime ledger's obscura tag reaching the release that carries this
fix.

It also forces a second decision downstream. Because the geometry cannot be trusted, Aleph's page
tree makes visibility a property of `computed` alone and never of the box: a node with no rectangle
stays visible, keeps its ref, and only loses its `@x,y wxh` suffix. That rule exists because two
measurements disagreed — inline `<a>`/`<span>` reported zero quads on obscura while the same
elements reported real boxes on Chromium — and neither could be generalized. A `captureSnapshot`
that reports real layout collapses that whole branch.

## Evidence

The narrative and the original anchors come from the read-only survey in
`docs/superpowers/specs/2026-09-06-browser-dual-engine-evidence/obscura-source-survey.md`, §3
("CDP server", and within it "Two geometry landmines over CDP", `:290`) and §4 ("Direct Rust access
to DOM + layout", `:308`), taken at `72c84ad`. **Every anchor printed above was re-read on
2026-09-20 in the working tree at `eec047a1`**, which is why the two columns differ.
