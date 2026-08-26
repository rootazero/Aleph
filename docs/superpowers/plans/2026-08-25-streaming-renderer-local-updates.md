# Streaming Renderer Local-Update Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop Panel and TUI from re-doing O(whole-transcript) or O(whole-revealed-message) work on every streamed token, without changing what's rendered.

**Architecture:** A new pure `shared_ui_logic::markdown_stream` module answers "how much of this accumulated text is safe to treat as frozen" (fence-complete, no dangling reference-link definition). Panel uses it to cache already-rendered HTML per message and stops the `<For>` list from remounting message bubbles on every token. TUI uses it to cache already-rendered `Line`s per message and stops redrawing the whole terminal frame on ticks that change nothing visible.

**Tech Stack:** Rust workspace; `shared/ui_logic` (crate `shared-ui-logic`, feature-gated leptos/wasm); Leptos 0.8 (Panel, `interfaces/webchat`); ratatui (TUI, `interfaces/tui`, crate `aleph-tui`).

**Spec:** `docs/superpowers/specs/2026-08-25-streaming-renderer-local-updates-design.md`

## Global Constraints

- Work happens in worktree branch `streaming-renderer-local-updates-2026-08-25`, never on `main` (the spec doc itself is the one deliberate exception, already committed to `main` — see spec header).
- `shared_ui_logic::markdown_stream` is the ONLY new module in this plan; no new workspace crate.
- `interfaces/tui`'s dependency on `shared-ui-logic` MUST use `default-features = false` (no leptos/wasm-bindgen/web-sys in a native binary).
- Correctness never regresses for a performance win: every boundary-unsafe case must fall back to full reprocessing, not skip rendering or panic.
- The first frame of a run (Panel: first token; TUI: first redraw after a run starts) must never be delayed by any caching/coalescing added here.
- Out of scope, do not touch: `timeline.rs::build_rows`'s full-refold, syntect/`render_markdown`'s completion-time cost, Approach B (structural freeze into real TUI scrollback / fully non-reactive Panel DOM) — see spec §7.
- Verification per task: `cargo test -p shared-ui-logic --lib`, `cargo test -p aleph-panel --lib` (NOT `cargo check` — this crate's `#[cfg(test)]` modules are invisible to `check`), `cargo test -p aleph-tui --lib`, `cargo clippy -p <touched-crate> -- -D warnings`.

---

### Task 1: `shared_ui_logic::markdown_stream` — fence-boundary core

**Files:**
- Create: `shared/ui_logic/src/markdown_stream/mod.rs`
- Create: `shared/ui_logic/src/markdown_stream/boundary.rs`
- Modify: `shared/ui_logic/src/lib.rs` — add `pub mod markdown_stream;`
- Test: inline `#[cfg(test)] mod tests` in `boundary.rs`

**Interfaces:**
- Produces: `pub fn safe_freeze_offset(text: &str, prev_safe: usize) -> Option<usize>` — given the full accumulated text and the last-known-safe byte offset, returns the new safe-to-freeze byte offset (always on a `\n` boundary), or `None` if no progress beyond `prev_safe` is possible. Consumed by Task 3 (Panel) and Task 6 (TUI).

First, check `shared/ui_logic/src/lib.rs` to see the existing top-level module list (`cat shared/ui_logic/src/lib.rs`) so the new `pub mod markdown_stream;` line lands next to the existing `pub mod state;`-style declarations, not duplicated or misplaced.

- [ ] **Step 1: Write the failing tests**

Create `shared/ui_logic/src/markdown_stream/boundary.rs`:

```rust
//! Streaming markdown boundary detection.
//!
//! Answers one question: given text that is still growing, how much of it can
//! be treated as frozen (safe to render once and never re-touch) right now?
//! "Safe" means the offset lands after a complete line, is not inside an
//! unclosed fenced code block, and is not immediately after a reference-link
//! definition (`[label]: ...`) that a following line could still extend.
//!
//! Deliberately conservative: every unsafe case returns less progress than a
//! perfectly precise parser might, never more. A caller that gets `None` (or
//! an offset short of what it hoped for) simply reprocesses that tail in
//! full — correctness never depends on this module being exactly right, only
//! on it never being wrong in the unsafe direction. Mirrors codex's own
//! documented escape hatch in `markdown_stream.rs` (`commit_complete_source`).

/// See module docs.
pub fn safe_freeze_offset(text: &str, prev_safe: usize) -> Option<usize> {
    let mut in_fence = false;
    let mut pending_ref_def = false;
    let mut last_safe = prev_safe;
    let mut cursor = prev_safe;

    for line in text[prev_safe..].split_inclusive('\n') {
        if !line.ends_with('\n') {
            // Incomplete trailing line (no newline yet) — never safe.
            break;
        }
        let trimmed = line.trim_end_matches('\n');
        cursor += line.len();

        if trimmed.trim_start().starts_with("```") {
            in_fence = !in_fence;
            if !in_fence {
                // Just closed a fence — safe up to and including this line.
                last_safe = cursor;
                pending_ref_def = false;
            }
            continue;
        }
        if in_fence {
            continue;
        }
        if trimmed.trim().is_empty() {
            // A blank line always ends any open paragraph or reference-link
            // definition, so it clears the pending flag and is itself safe.
            last_safe = cursor;
            pending_ref_def = false;
            continue;
        }
        if is_reference_link_def_start(trimmed) {
            pending_ref_def = true;
            continue;
        }
        if pending_ref_def {
            // Still inside a possible definition continuation — don't
            // advance past it until a blank line confirms it's closed.
            continue;
        }
        last_safe = cursor;
    }

    if last_safe > prev_safe {
        Some(last_safe)
    } else {
        None
    }
}

/// A line that could start a CommonMark reference-link definition:
/// `[label]: destination "optional title"`. Deliberately loose (doesn't
/// validate the destination) — false positives just cost a forfeited perf
/// win, never a correctness bug.
fn is_reference_link_def_start(trimmed_line: &str) -> bool {
    let after_indent = trimmed_line.trim_start();
    after_indent.starts_with('[') && after_indent.contains("]:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_fence_freezes_up_to_last_complete_line() {
        let text = "line one\nline two\nline three"; // no trailing \n on last line
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, Some("line one\nline two\n".len()));
    }

    #[test]
    fn open_fence_blocks_freezing_past_fence_start() {
        let text = "before\n```rust\nfn main() {}\n";
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, Some("before\n".len()));
    }

    #[test]
    fn closed_fence_allows_freezing_through_it_and_beyond() {
        let text = "before\n```rust\ncode\n```\nafter\n";
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, Some(text.len()));
    }

    #[test]
    fn no_progress_returns_none() {
        let text = "```rust\n";
        assert_eq!(safe_freeze_offset(text, 0), None);
    }

    #[test]
    fn incremental_call_resumes_from_prev_safe() {
        let text = "line one\nline two\n";
        let first = safe_freeze_offset(text, 0).unwrap();
        assert_eq!(first, "line one\n".len());
        let grown = "line one\nline two\nline three\n";
        let second = safe_freeze_offset(grown, first).unwrap();
        assert_eq!(second, grown.len());
    }

    #[test]
    fn fence_state_does_not_leak_across_a_resumed_call() {
        // prev_safe always lands outside a fence by construction, so a
        // resumed call must not spuriously believe it starts inside one.
        let text = "```rust\ncode\n```\nmore\n";
        let after_fence = safe_freeze_offset(text, 0).unwrap();
        assert_eq!(after_fence, text.len());
    }
}
```

Create `shared/ui_logic/src/markdown_stream/mod.rs`:

```rust
//! Streaming markdown boundary detection — see [`boundary`] for the
//! algorithm and its safety rationale. Shared between Panel (HTML renderer)
//! and TUI (ratatui `Line` renderer): only the "how far is it safe to
//! freeze" decision is shared, not the rendering.

mod boundary;

pub use boundary::safe_freeze_offset;
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p shared-ui-logic --lib markdown_stream -- --nocapture`
Expected: FAIL with "unresolved module `markdown_stream`" (module not wired into `lib.rs` yet) or "cannot find function" if `lib.rs` already has the `pub mod` line — in that case the tests themselves should compile and pass immediately since the implementation above is written in the same step. If they pass immediately, that's expected here (this task writes test+impl together per the "boundary rule" nature of this module, where the algorithm has no simpler strawman); confirm by temporarily commenting out the loop body and re-running to see a real failure, then restore it.

- [ ] **Step 3: Wire the module into `lib.rs`**

Read `shared/ui_logic/src/lib.rs` first. Add `pub mod markdown_stream;` alongside the existing `pub mod` declarations (do not reorder existing ones).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p shared-ui-logic --lib markdown_stream`
Expected: PASS, all 6 tests.

- [ ] **Step 5: Lint**

Run: `cargo clippy -p shared-ui-logic -- -D warnings`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add shared/ui_logic/src/markdown_stream/ shared/ui_logic/src/lib.rs
git commit -m "feat: add markdown_stream fence-boundary detection to shared-ui-logic"
```

---

### Task 2: `markdown_stream` — reference-link-definition safety refinement

**Files:**
- Modify: `shared/ui_logic/src/markdown_stream/boundary.rs` (tests only — the implementation from Task 1 already includes `pending_ref_def` handling; this task is the dedicated regression coverage for it)

**Interfaces:**
- Consumes: `safe_freeze_offset` from Task 1 (unchanged signature).
- Produces: nothing new — this task exists because Task 1's `pending_ref_def` logic needs its own test cycle per "smallest unit worth a fresh reviewer's gate" (a reviewer could reasonably want to see this edge case proven separately from the fence-only cases).

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `shared/ui_logic/src/markdown_stream/boundary.rs`:

```rust
    #[test]
    fn dangling_reference_link_def_pins_the_freeze_point() {
        let text = "See [foo] below.\n\n[foo]: https://example.com\nmore text\n";
        let result = safe_freeze_offset(text, 0);
        // Safe only through the blank line before the definition — the
        // definition line and everything after it stay unfrozen because no
        // blank line has confirmed the definition is closed.
        assert_eq!(result, Some("See [foo] below.\n\n".len()));
    }

    #[test]
    fn blank_line_after_reference_link_def_clears_the_pin() {
        let text = "See [foo] below.\n\n[foo]: https://example.com\n\nmore text\n";
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, Some(text.len()));
    }

    #[test]
    fn reference_link_def_inside_a_fence_is_just_code() {
        // A `[x]:`-shaped line inside a fence is code content, not a real
        // reference-link definition — the fence rule takes priority.
        let text = "```\n[foo]: not a real link def, just code\n```\nafter\n";
        let result = safe_freeze_offset(text, 0);
        assert_eq!(result, Some(text.len()));
    }
```

- [ ] **Step 2: Run tests to verify they were already implemented correctly (or fail if not)**

Run: `cargo test -p shared-ui-logic --lib markdown_stream`
Expected: PASS if Task 1's `pending_ref_def` logic is correct as written above. If any of the 3 new tests FAIL, fix `boundary.rs`'s `safe_freeze_offset` (most likely cause: the `pending_ref_def` check ordering relative to the fence check) until they pass — do not weaken the test assertions to match broken behavior.

- [ ] **Step 3: Confirm full suite green**

Run: `cargo test -p shared-ui-logic --lib`
Expected: PASS, 9 tests total.

- [ ] **Step 4: Commit**

```bash
git add shared/ui_logic/src/markdown_stream/boundary.rs
git commit -m "test: cover reference-link-definition boundary safety in markdown_stream"
```

---

### Task 3: Panel — stable-prefix HTML cache in `TypewriterClock`

**Files:**
- Modify: `interfaces/webchat/src/state/typewriter.rs`
- Modify: `interfaces/webchat/src/components/markdown.rs`
- Modify: `interfaces/webchat/Cargo.toml` — no change needed, `shared-ui-logic` is already a dependency with `leptos`/`wasm` features (confirmed at `interfaces/webchat/Cargo.toml:109`).
- Test: inline `#[cfg(test)]` modules in both files above.

**Interfaces:**
- Consumes: `shared_ui_logic::markdown_stream::safe_freeze_offset` (Task 1/2).
- Produces: `TypewriterClock::stable_prefix_for(&self, id: &str) -> Option<(String, usize)>`, `TypewriterClock::set_stable_prefix(&self, id: &str, html: String, safe_offset: usize)`, `TypewriterClock::clear_stable_prefix(&self, id: &str)` — consumed by `TypewriterRenderer` in this same task (no other task depends on these).

- [ ] **Step 1: Write the failing test for the new `TypewriterClock` methods**

Add to the `tests` module in `interfaces/webchat/src/state/typewriter.rs` (near the existing `prune_stale_*` tests):

```rust
    #[test]
    fn stable_prefix_round_trips() {
        let clock = TypewriterClock::new();
        assert_eq!(clock.stable_prefix_for("m1"), None);
        clock.set_stable_prefix("m1", "<p>hi</p>".to_string(), 5);
        assert_eq!(
            clock.stable_prefix_for("m1"),
            Some(("<p>hi</p>".to_string(), 5))
        );
    }

    #[test]
    fn finish_clears_the_stable_prefix_too() {
        let clock = TypewriterClock::new();
        clock.set_stable_prefix("m1", "<p>hi</p>".to_string(), 5);
        clock.finish("m1");
        assert_eq!(clock.stable_prefix_for("m1"), None);
    }

    #[test]
    fn clear_stable_prefix_is_a_no_op_on_a_missing_id() {
        let clock = TypewriterClock::new();
        clock.clear_stable_prefix("does-not-exist"); // must not panic
    }

    #[test]
    fn stale_cursor_pruning_also_drops_its_stable_prefix() {
        let clock = TypewriterClock::new();
        // First sight of "orphan" — advance_for creates a cursor.
        clock.advance_for("orphan", 10, 0.0, 200, false);
        clock.set_stable_prefix("orphan", "<p>partial</p>".to_string(), 4);
        // Advance a different id far enough in the future that "orphan" is
        // stale (> STALE_CURSOR_MS old) — this triggers prune_stale.
        clock.advance_for("fresh", 10, 100_000.0, 200, false);
        assert_eq!(clock.stable_prefix_for("orphan"), None);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p aleph-panel --lib typewriter -- --nocapture`
Expected: FAIL with "no method named `stable_prefix_for`/`set_stable_prefix`/`clear_stable_prefix` found".

- [ ] **Step 3: Implement the `TypewriterClock` additions**

In `interfaces/webchat/src/state/typewriter.rs`, add a field to the `TypewriterClock` struct (after the existing `reveals` field):

```rust
    /// `message_id → (already-rendered HTML for the safe prefix, chars that
    /// prefix represents)`. Separate from `reveals` (not folded into
    /// [`Reveal`]) because `Reveal` is deliberately `Copy`/allocation-free;
    /// this cache holds an owned `String` and is invalidated independently
    /// (see [`TypewriterClock::clear_stable_prefix`]).
    stable_prefixes: RwSignal<std::collections::HashMap<String, (String, usize)>>,
```

Update `TypewriterClock::new()` to initialize it:

```rust
            stable_prefixes: RwSignal::new(HashMap::new()),
```

Add the three methods to `impl TypewriterClock`:

```rust
    /// Cached `(html, safe_offset)` for `id`'s already-rendered prefix, if
    /// any. `safe_offset` is a byte offset into the message's content, per
    /// [`shared_ui_logic::markdown_stream::safe_freeze_offset`].
    #[must_use]
    pub fn stable_prefix_for(&self, id: &str) -> Option<(String, usize)> {
        self.stable_prefixes.with_untracked(|m| m.get(id).cloned())
    }

    /// Replace `id`'s cached stable prefix.
    pub fn set_stable_prefix(&self, id: &str, html: String, safe_offset: usize) {
        self.stable_prefixes
            .update_untracked(|m| { m.insert(id.to_string(), (html, safe_offset)); });
    }

    /// Drop `id`'s cached stable prefix. Called from [`Self::finish`] and
    /// whenever a caller observes `is_streaming == false` for a still-
    /// sweeping message: `finalize_answer`/`set_step_text` can swap a
    /// message's `content` wholesale rather than append to it, and a cached
    /// HTML prefix computed against the old content would then describe text
    /// that no longer exists at that offset.
    pub fn clear_stable_prefix(&self, id: &str) {
        self.stable_prefixes.update_untracked(|m| { m.remove(id); });
    }
```

Update `finish()` to also clear the stable prefix:

```rust
    pub fn finish(&self, id: &str) {
        self.reveals.update_untracked(|m| {
            m.remove(id);
        });
        self.clear_stable_prefix(id);
    }
```

Change `prune_stale`'s signature to report what it removed, and thread that through `advance_for`:

```rust
fn prune_stale(map: &mut HashMap<String, Reveal>, now: f64) -> Vec<String> {
    let stale: Vec<String> = map
        .iter()
        .filter(|(_, r)| {
            (now - r.last_ms).partial_cmp(&STALE_CURSOR_MS) == Some(std::cmp::Ordering::Greater)
        })
        .map(|(k, _)| k.clone())
        .collect();
    for k in &stale {
        map.remove(k);
    }
    stale
}
```

In `advance_for`, replace the `self.reveals.update_untracked(...)` block:

```rust
        let mut pruned: Vec<String> = Vec::new();
        self.reveals.update_untracked(|m| {
            if is_new {
                pruned = prune_stale(m, now);
            }
            m.insert(id.to_string(), next);
        });
        if !pruned.is_empty() {
            self.stable_prefixes.update_untracked(|m| {
                for k in &pruned {
                    m.remove(k);
                }
            });
        }
        next.revealed
```

(The two existing `prune_stale` tests, `prune_stale_drops_only_abandoned_cursors` and `prune_stale_is_a_no_op_when_the_clock_goes_backwards`, call `prune_stale(&mut map, now)` and only assert on `map` afterward — they still compile and pass unchanged since discarding the new `Vec<String>` return value is not an error in Rust.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p aleph-panel --lib typewriter`
Expected: PASS, all tests (existing + 4 new).

- [ ] **Step 5: Write the failing test for `TypewriterRenderer`'s incremental use of the cache**

`TypewriterRenderer` is a Leptos `#[component]`, not directly unit-testable without a reactive runtime. Extract the pure part — turning `(cached_html, cached_offset, revealed_prefix)` into `(new_html, new_offset)` — into a standalone function first, so it's testable without Leptos. Add to `interfaces/webchat/src/components/markdown.rs`, near `render_streaming`:

```rust
/// Extend a cached stable-prefix render with newly-revealed text.
///
/// `revealed_prefix` is the full text revealed so far (`content` truncated
/// to the typewriter's current `revealed` char count) — NOT just the new
/// characters, since [`shared_ui_logic::markdown_stream::safe_freeze_offset`]
/// needs to see any fence/reference-link-def state spanning the cached
/// boundary and the newly-revealed text. Returns the extended
/// `(html, new_safe_offset)`; when no further progress is safe, `html` is
/// unchanged and `new_safe_offset == cached_offset`.
fn extend_stable_prefix(
    cached_html: &str,
    cached_offset: usize,
    revealed_prefix: &str,
) -> (String, usize) {
    match shared_ui_logic::markdown_stream::safe_freeze_offset(revealed_prefix, cached_offset) {
        Some(new_offset) if new_offset > cached_offset => {
            let delta = &revealed_prefix[cached_offset..new_offset];
            let mut html = cached_html.to_string();
            html.push_str(&render_streaming(delta));
            (html, new_offset)
        }
        _ => (cached_html.to_string(), cached_offset),
    }
}
```

Add tests in `components/markdown.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn extend_stable_prefix_appends_only_the_new_safe_delta() {
        let (html, offset) = extend_stable_prefix("", 0, "line one\nline two\n");
        assert_eq!(offset, "line one\nline two\n".len());
        assert!(html.contains("line one"));
        assert!(html.contains("line two"));

        // Simulate the next tick: more text arrived, cache reused.
        let (html2, offset2) =
            extend_stable_prefix(&html, offset, "line one\nline two\nline three\n");
        assert_eq!(offset2, "line one\nline two\nline three\n".len());
        assert!(html2.starts_with(&html), "must extend, not rebuild");
        assert!(html2.contains("line three"));
    }

    #[test]
    fn extend_stable_prefix_no_ops_when_no_safe_progress_exists() {
        let (html, offset) = extend_stable_prefix("<cached>", 3, "```rust\n");
        assert_eq!(html, "<cached>");
        assert_eq!(offset, 3);
    }
```

- [ ] **Step 6: Run tests to verify they fail, then pass**

Run: `cargo test -p aleph-panel --lib markdown::tests::extend_stable_prefix -- --nocapture`
Expected: first FAIL ("cannot find function `extend_stable_prefix`"), then after adding the function above, PASS.

- [ ] **Step 7: Wire `extend_stable_prefix` into `TypewriterRenderer`'s sweeping branch**

In `TypewriterRenderer` (`components/markdown.rs`), replace the "Still sweeping" branch (the `else` arm after `if revealed >= total`):

```rust
        } else {
            // Still sweeping — advance on each ~30fps animation tick.
            clock.tick.track();
            let id = message_id.get_value();
            if !is_streaming {
                // Reveal hasn't caught up but the stream already ended —
                // finalize may have swapped `content` wholesale, so a cached
                // prefix could describe text that's no longer there. Drop it
                // and fall back to an uncached render for this tick; the
                // cache rebuilds itself from the next call onward.
                clock.clear_stable_prefix(&id);
                return content.with_value(|c| {
                    let shown: String = c.chars().take(revealed).collect();
                    render_streaming_with_cursor(&shown)
                });
            }
            content.with_value(|c| {
                let revealed_prefix: String = c.chars().take(revealed).collect();
                let (cached_html, cached_offset) =
                    clock.stable_prefix_for(&id).unwrap_or_default();
                let (html, safe_offset) =
                    extend_stable_prefix(&cached_html, cached_offset, &revealed_prefix);
                if safe_offset != cached_offset {
                    clock.set_stable_prefix(&id, html.clone(), safe_offset);
                }
                let tail = &revealed_prefix[safe_offset..];
                format!("{html}{}{STREAMING_CURSOR_HTML}", render_streaming(tail))
            })
        }
```

- [ ] **Step 8: Run the full Panel test suite**

Run: `cargo test -p aleph-panel --lib`
Expected: PASS, no regressions.

- [ ] **Step 9: Lint**

Run: `cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings`
Expected: clean.

- [ ] **Step 10: Manual smoke check (per Global Constraints — correctness over perf win)**

Run `just dev`, open Panel, send a message that produces a multi-paragraph response with at least one fenced code block, and confirm: the typewriter reveal still animates smoothly, the code block still renders with correct fencing (no leaked backticks, no missing highlighting after completion), and clicking to skip still jumps to the full text.

- [ ] **Step 11: Commit**

```bash
git add interfaces/webchat/src/state/typewriter.rs interfaces/webchat/src/components/markdown.rs
git commit -m "feat: cache the stable-prefix HTML render in TypewriterClock, extend only the tail"
```

---

### Task 4: Panel — stable `<For>` row identity for streaming bubbles

**Files:**
- Modify: `interfaces/webchat/src/platform/wide/views/chat/timeline.rs`
- Modify: `interfaces/webchat/src/platform/wide/views/chat/messages.rs`
- Test: inline in `timeline.rs`'s existing `#[cfg(test)] mod tests`.

**Interfaces:**
- Consumes: nothing from earlier tasks (independent of Tasks 1-3; safe to implement before or after them — listed after for narrative flow only).
- Produces: `TimelineRow::Message` and `TimelineRow::Narration` new shapes (below), consumed only within this task (`messages.rs`'s render dispatch).

This task changes `TimelineRow::Message`/`Narration` from carrying an owned `ChatMessage` snapshot to carrying `id` plus the small structural facts needed for `<For>` keying and one-time branch selection — **not** the growing `content` field. `MessageBubble` (the component that actually renders content) is converted from a plain owned-`ChatMessage` prop to a reactive per-row lookup, so it keeps rendering current content after the row stops remounting.

- [ ] **Step 1: Write the failing tests for the new `TimelineRow` shape**

In `timeline.rs`'s test module, replace these existing tests (they currently assert the OLD behavior — key changes on content growth — which this task deliberately reverses) and add new ones. First, **read `timeline.rs` lines 1-260 and 750-970 in full** (you already have the context from this plan's research, but re-read before editing so line numbers match your working copy exactly).

Replace `row_key_narration_changes_on_content_growth` with:

```rust
    #[test]
    fn row_key_narration_is_stable_across_content_growth() {
        // Content growth alone (the common case: a token arriving mid-stream)
        // must NOT change the key — that's what let the DOM subtree survive
        // across tokens instead of remounting every one.
        let m1 = msg_step("intermediate-r1-1", 1, "partial", true);
        let m2 = msg_step("intermediate-r1-1", 1, "partial more", true);
        let rows1 = vec![TimelineRow::Narration { id: m1.id.clone(), is_streaming: m1.is_streaming }];
        let rows2 = vec![TimelineRow::Narration { id: m2.id.clone(), is_streaming: m2.is_streaming }];
        assert_eq!(row_key(&rows1[0]), row_key(&rows2[0]));
    }

    #[test]
    fn row_key_narration_changes_when_streaming_ends() {
        let m1 = msg_step("intermediate-r1-1", 1, "text", true);
        let mut m2 = m1.clone();
        m2.is_streaming = false;
        assert_ne!(
            row_key(&TimelineRow::Narration { id: m1.id.clone(), is_streaming: m1.is_streaming }),
            row_key(&TimelineRow::Narration { id: m2.id.clone(), is_streaming: m2.is_streaming })
        );
    }
```

Update the remaining call sites that construct or match `TimelineRow::Message { message, clock }` / `TimelineRow::Narration { message }` in the test module (lines noted from this plan's research — verify against your working copy):
- `empty_streaming_placeholder_emits_cursor_narration` (was line 764): change the pattern to `[TimelineRow::Narration { is_streaming, .. }] if *is_streaming`.
- `final_answer_and_user_stay_message_rows` (was line 788): change the pattern to `TimelineRow::Message { id, tool_call_count, .. } if id == "assistant-r-r" && *tool_call_count > 0`.
- `first_dated_message_gets_a_separator` (was line 842): change the match arm to `TimelineRow::Message { id, clock, .. } => { assert_eq!(id, "a"); assert_eq!(clock, "T1500"); }`.
- `undated_messages_emit_no_separator_and_empty_clock` (was line 890): change to `TimelineRow::Message { clock, .. } => assert!(clock.is_empty())`.
- The `TimelineRow::Message { .. }` / `TimelineRow::Narration { .. }` wildcard matches (lines ~579, 580, 705, 862, 863, 903, 905) need no changes — they already only match on the variant, not its fields.
- The direct constructor at line ~967 (`let m = TimelineRow::Message { ... }`): update field names to the new shape.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p aleph-panel --lib timeline -- --nocapture`
Expected: FAIL to compile (field name mismatches) — expected at this point, since the enum hasn't changed yet.

- [ ] **Step 3: Change the `TimelineRow` enum**

In `timeline.rs`, replace:

```rust
    Message { message: ChatMessage, clock: String },
```

with:

```rust
    /// A message plus its resolved clock label and the small structural
    /// facts a row needs before it can render — NOT the growing `content`.
    /// `<For>`'s children closure runs once per stable key (see `row_key`);
    /// content itself is fetched reactively inside the rendered component so
    /// it keeps updating without a remount. `has_plan_archive`/`role` gate
    /// which component this row renders as (`PlanArchiveCell` /
    /// `SystemNoticeRow` / `ToolFallbackRow` / `MessageBubble`) — a decision
    /// made once, at closure-run time, since a message's role/archive-ness
    /// doesn't change after creation.
    Message {
        id: String,
        role: String,
        has_plan_archive: bool,
        is_streaming: bool,
        is_intermediate: bool,
        tool_call_count: usize,
        has_model_info: bool,
        clock: String,
    },
```

and replace:

```rust
    Narration { message: ChatMessage },
```

with:

```rust
    /// An intermediate turn's narration text — see the variant's original
    /// doc above for what it covers. Carries only what `row_key` and the
    /// rendered `NarrationRow` need reactively (just `id`); content is
    /// fetched the same way as `Message` rows.
    Narration { id: String, is_streaming: bool },
```

- [ ] **Step 4: Update `build_rows`' construction sites**

Change (was around line 90):

```rust
                rows.push(TimelineRow::Narration { message: m.clone() });
```

to:

```rust
                rows.push(TimelineRow::Narration { id: m.id.clone(), is_streaming: m.is_streaming });
```

Change (verified at `timeline.rs:124-141`, inside `build_rows`'s main loop, the non-step-row branch):

```rust
        let clock = match m.timestamp {
            Some(ts) => {
                let day = day_ordinal(ts);
                if last_day != Some(day) {
                    rows.push(TimelineRow::DaySeparator {
                        key: day.to_string(),
                        label: label_for(ts),
                    });
                    last_day = Some(day);
                }
                clock_for(ts)
            }
            None => String::new(),
        };
        rows.push(TimelineRow::Message {
            id: m.id.clone(),
            role: m.role.clone(),
            has_plan_archive: m.plan_archive.is_some(),
            is_streaming: m.is_streaming,
            is_intermediate: m.is_intermediate,
            tool_call_count: m.tool_calls.len(),
            has_model_info: m.model_info.is_some(),
            clock,
        });
```

(Only the final `rows.push(...)` call changes shape; the `let clock = match m.timestamp { ... }` block above it — including the day-separator push — is unchanged.)

- [ ] **Step 5: Update `row_key`**

First fix the function's doc comment (verified at `timeline.rs:227-230`) — it currently describes the exact per-token remount behavior this task removes, and would otherwise contradict the code right below it:

```rust
/// Stable `<For>` key for a timeline row.
///
/// A streaming `Message`/`Narration` row's key does NOT include content
/// length — it stays stable while content grows so the row's DOM subtree is
/// never unmounted/remounted per token (see `messages.rs`'s per-row `Memo`
/// lookup for how the rendered content still updates without a remount).
/// The key changes only on a structural transition (streaming ends, a tool
/// call is added, etc.); separators key on their day.
```

Replace the `Message` and `Narration` arms:

```rust
        TimelineRow::Message {
            id,
            is_streaming,
            is_intermediate,
            tool_call_count,
            has_model_info,
            clock,
            ..
        } => format!(
            "{id}:{is_streaming}:{is_intermediate}:{tool_call_count}:{has_model_info}:{clock}",
        ),
        TimelineRow::Narration { id, is_streaming } => format!("narr:{id}:{is_streaming}"),
```

(Note: `content.len()` is gone from both — that's the fix. `role`/`has_plan_archive` are deliberately NOT in the key: they don't change after a message is created, so including them would be extra key churn for zero benefit.)

- [ ] **Step 6: Run tests, fix remaining compile errors**

Run: `cargo test -p aleph-panel --lib timeline`
Fix any remaining field-name mismatches in the test module (this plan's research listed the known call sites in Step 1 above; there may be one or two more your working copy's exact state reveals — apply the same field-renaming pattern).
Expected: PASS, all tests in `timeline.rs`.

- [ ] **Step 7: Convert `messages.rs`'s row dispatch and `MessageBubble` to reactive per-row lookup**

Read `messages.rs` lines 300-360 (the `<For>` children closure) and lines 658-1005 (`MessageBubble`'s full body) in your working copy before editing.

Replace the `TimelineRow::Message { message, clock } => { ... }` arm (was lines 315-335) with:

```rust
                                    TimelineRow::Message { id, has_plan_archive, role, clock, .. } => {
                                        if has_plan_archive {
                                            let lookup_id = id.clone();
                                            let snapshot = chat.messages.with_untracked(|m| {
                                                m.iter().find(|x| x.id == lookup_id).cloned()
                                            });
                                            match snapshot.and_then(|m| m.plan_archive.clone()) {
                                                Some(p) => view! { <PlanArchiveCell plan=p /> }.into_any(),
                                                None => view! {}.into_any(),
                                            }
                                        } else if role == "system" {
                                            let lookup_id = id.clone();
                                            let snapshot = chat.messages.with_untracked(|m| {
                                                m.iter().find(|x| x.id == lookup_id).cloned()
                                            }).unwrap_or_default(); // ChatMessage must derive/implement Default, or use a documented placeholder if it does not — check before assuming
                                            view! { <SystemNoticeRow message=snapshot /> }.into_any()
                                        } else if role == "tool" {
                                            let lookup_id = id.clone();
                                            let snapshot = chat.messages.with_untracked(|m| {
                                                m.iter().find(|x| x.id == lookup_id).cloned()
                                            }).unwrap_or_default();
                                            view! { <ToolFallbackRow message=snapshot /> }.into_any()
                                        } else {
                                            let lookup_id = id.clone();
                                            let message = Memo::new(move |_| {
                                                chat.messages.with(|m| m.iter().find(|x| x.id == lookup_id).cloned())
                                            });
                                            view! { <MessageBubble message=message clock=clock /> }.into_any()
                                        }
                                    }
```

Before using `.unwrap_or_default()` above: check whether `ChatMessage` implements `Default`. If it does not, replace those two branches' fallback with an explicit `match snapshot { Some(m) => ..., None => view! {}.into_any() }` instead — do not add a `Default` impl as a side effect of this task (out of scope; ask before adding derives to a shared struct).

Replace the `TimelineRow::Narration { message } => ...` arm with:

```rust
                                    TimelineRow::Narration { id, .. } => {
                                        let lookup_id = id.clone();
                                        let message = Memo::new(move |_| {
                                            chat.messages.with(|m| m.iter().find(|x| x.id == lookup_id).cloned())
                                        });
                                        view! { <NarrationRow message=message /> }.into_any()
                                    }
```

Now convert `MessageBubble`'s signature and body. Change:

```rust
fn MessageBubble(message: ChatMessage, clock: String) -> impl IntoView {
```

to:

```rust
fn MessageBubble(message: Memo<Option<ChatMessage>>, clock: String) -> impl IntoView {
```

Convert every subsequent `message.<field>` read in the function body to a reactive read. Apply this pattern to each one (worked examples for the first three; apply the same shape to the rest as you find them while reading the function top to bottom):

```rust
    // Before: let is_user = message.role == "user";
    let is_user = move || message.with(|m| m.as_ref().is_some_and(|m| m.role == "user"));

    // Before: let has_error = message.error.is_some();
    let has_error = move || message.with(|m| m.as_ref().is_some_and(|m| m.error.is_some()));

    // Before: let has_tools = !message.tool_calls.is_empty();
    let has_tools = move || message.with(|m| m.as_ref().is_some_and(|m| !m.tool_calls.is_empty()));
```

Any place that previously used `is_user`/`has_error`/`has_tools`/etc. as a plain `bool` in a non-reactive context (e.g. deciding `bubble_align` once) now needs to call the closure: `is_user()` instead of `is_user`. Where the original code built a `view!` fragment that embedded one of these values directly, wrap it in a `move ||` reactive closure so Leptos re-evaluates it when `message` changes (Leptos views already support `{move || ...}` children — use that form, don't restructure the surrounding `view!` macro beyond adding the closure).

For the two `TypewriterRenderer` call sites (content/message_id/is_streaming props), change:

```rust
                                <TypewriterRenderer content=content message_id=message_id is_streaming=is_streaming />
```

to a reactive form — since `TypewriterRenderer` itself still takes plain `content: String, message_id: String, is_streaming: bool` props (unchanged by this task — Task 3 already made its internals track changes via the clock, but its own re-invocation still needs to happen on `message` changes), wrap the whole `view!` block that constructs it in a `move ||`:

```rust
                                {move || message.with(|m| m.as_ref().map(|m| {
                                    let content = m.content.clone();
                                    let message_id = timeline::reveal_key(m);
                                    let is_streaming = m.is_streaming;
                                    view! { <TypewriterRenderer content=content message_id=message_id is_streaming=is_streaming /> }
                                }))}
```

Do this for both call sites (the team-layout branch and the original-layout branch).

Handle the `None` case (id not found — should not happen per Decision in the spec, but must not panic): wherever `message.with(|m| m.as_ref()...)` is used and the surrounding code needs a value unconditionally (not `Option`-aware), guard with a top-level early return inside the component:

```rust
    // Early exit if the row's id no longer resolves to a message (should not
    // happen — ids are stamped once — but a lookup miss must render nothing
    // rather than panic).
    let exists = move || message.with(|m| m.is_some());
```

and wrap the component's whole `view!` output in `{move || exists().then(|| /* existing view! block */)}` if the existing body assumes `message` is always present. (Read the actual body to decide the minimal-diff way to add this guard — the exact placement depends on how many places destructure `message` at the top vs. inline.)

- [ ] **Step 8: Remove the now-unnecessary entrance-animation gate**

Find (was lines ~892-899 in `messages.rs`):

```rust
    // One-shot rise+fade as the bubble mounts. Gated to non-streaming: the
    // keyed <For> recreates a streaming bubble on every token, so applying
    // the entrance there would replay it per chunk. User + finalized
    // assistant bubbles mount once, so it plays exactly once.
    let wrapper_class = if is_streaming {
        format!("{bubble_align} group relative")
    } else {
        format!("{bubble_align} group relative aleph-msg-in")
    };
```

Replace with (bubbles no longer remount per token, so the animation now plays exactly once regardless of streaming state — note `bubble_align`/`is_streaming` are now reactive closures per Step 7, adjust the call sites accordingly):

```rust
    // One-shot rise+fade as the bubble mounts. Safe unconditionally now: the
    // row no longer remounts per token (stable `<For>` key, see
    // `timeline::row_key`), so this only ever plays once per bubble.
    let wrapper_class = move || format!("{} group relative aleph-msg-in", bubble_align());
```

- [ ] **Step 9: Run the full Panel test suite**

Run: `cargo test -p aleph-panel --lib`
Expected: PASS. Fix any remaining reactive-conversion compile errors by applying the same closure-wrapping pattern from Step 7 to whatever field reads the compiler flags.

- [ ] **Step 10: Lint**

Run: `cargo clippy -p aleph-panel --target wasm32-unknown-unknown -- -D warnings`

- [ ] **Step 11: Manual smoke check**

Run `just dev`. Send a message and confirm: (a) the streaming bubble no longer visibly "flickers"/loses hover state per token, (b) the entrance animation plays exactly once per bubble (not replayed, not skipped), (c) a system notice and a tool-fallback row (if you can trigger one) still render correctly, (d) switching conversations and re-sending still works (no stale `id` lookups).

- [ ] **Step 12: Commit**

```bash
git add interfaces/webchat/src/platform/wide/views/chat/timeline.rs interfaces/webchat/src/platform/wide/views/chat/messages.rs
git commit -m "fix: stop remounting the streaming message bubble on every token

TimelineRow::Message/Narration now carry id + structural facts instead of
an owned ChatMessage snapshot, so the <For> key stays stable while a
message streams. MessageBubble/NarrationRow fetch content reactively via
a per-row Memo instead of a captured value, so they keep updating without
a remount. Removes the is_streaming gate on the entrance animation, which
existed only to work around the remount this fixes."
```

---

### Task 5: TUI — add `shared-ui-logic` dependency + per-message line cache

**Files:**
- Modify: `interfaces/tui/Cargo.toml`
- Modify: `interfaces/tui/src/tui/widgets/chat_area.rs`
- Test: inline in `chat_area.rs`'s existing `#[cfg(test)] mod tests`.

**Interfaces:**
- Consumes: nothing from `shared_ui_logic::markdown_stream` yet (that's Task 6) — this task's cache is a coarse whole-message cache, valid independent of the boundary module.
- Produces: nothing new consumed elsewhere — self-contained.

- [ ] **Step 1: Add the dependency**

In `interfaces/tui/Cargo.toml`, add under `[dependencies]`:

```toml
shared-ui-logic = { path = "../../shared/ui_logic", default-features = false }
```

Run: `cargo check -p aleph-tui`
Expected: succeeds, and confirm no `leptos`/`wasm-bindgen`/`web-sys` entries appear in `cargo tree -p aleph-tui | grep -iE "leptos|wasm-bindgen|web-sys"` (expected: empty output).

- [ ] **Step 2: Write the failing test for cache-hit/cache-miss behavior**

Add to `chat_area.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn build_all_lines_reuses_cached_lines_for_unchanged_messages() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.add_user_message("Hello".into());
        state.ensure_assistant_message();
        if let ChatMessage::Assistant { content, is_streaming, .. } = state.current_assistant_mut() {
            content.push_str("Hi there!");
            *is_streaming = false;
        }

        let mut cache = LineCache::default();
        let first = build_all_lines_cached(&state.messages, state.verbose, state.spinner_frame, 80, &mut cache);
        let second = build_all_lines_cached(&state.messages, state.verbose, state.spinner_frame, 80, &mut cache);
        assert_eq!(first, second);
        // Cache must actually have been populated, not silently bypassed.
        assert!(!cache.entries.is_empty());
    }

    #[test]
    fn build_all_lines_invalidates_on_content_change() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.ensure_assistant_message();
        let mut cache = LineCache::default();
        let _ = build_all_lines_cached(&state.messages, state.verbose, state.spinner_frame, 80, &mut cache);
        if let ChatMessage::Assistant { content, .. } = state.current_assistant_mut() {
            content.push_str("new text");
        }
        let updated = build_all_lines_cached(&state.messages, state.verbose, state.spinner_frame, 80, &mut cache);
        let has_new_text = updated.iter().any(|line| {
            line.spans.iter().any(|s| s.content.as_ref().contains("new text"))
        });
        assert!(has_new_text, "changed content must not serve a stale cache entry");
    }

    #[test]
    fn build_all_lines_invalidates_on_width_change() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.add_system_message("x".repeat(60));
        let mut cache = LineCache::default();
        let wide = build_all_lines_cached(&state.messages, state.verbose, state.spinner_frame, 80, &mut cache);
        let narrow = build_all_lines_cached(&state.messages, state.verbose, state.spinner_frame, 20, &mut cache);
        assert_ne!(wide.len(), narrow.len(), "resize must reformat, not reuse the wide cache");
    }

    #[test]
    fn build_all_lines_cached_matches_uncached_output() {
        let mut state = AppState::new("test".into(), "claude".into());
        state.add_user_message("Hello".into());
        state.ensure_assistant_message();
        if let ChatMessage::Assistant { content, .. } = state.current_assistant_mut() {
            content.push_str("Hi there!");
        }
        let mut cache = LineCache::default();
        let cached = build_all_lines_cached(&state.messages, state.verbose, state.spinner_frame, 80, &mut cache);
        let uncached = build_all_lines(&state, 80);
        assert_eq!(cached, uncached, "caching must not change what's rendered");
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p aleph-tui --lib chat_area -- --nocapture`
Expected: FAIL — `LineCache`/`build_all_lines_cached` don't exist yet.

- [ ] **Step 4: Implement the cache**

In `chat_area.rs`, add (near the top, after imports):

```rust
use std::collections::HashMap;

/// Per-message rendered-line cache, owned by `AppState` across frames (see
/// `render_chat_area`'s caller in `render.rs` for where it's threaded
/// through). Keyed by the message's index in `state.messages` — safe because
/// a cache entry also validates against the message's own variant kind and
/// content length before being trusted (see `build_all_lines_cached`); a
/// coincidental `(kind, len)` match at a shifted index is the only failure
/// mode, and it self-heals the next frame once content actually diverges.
#[derive(Default)]
pub struct LineCache {
    entries: HashMap<usize, CachedEntry>,
}

struct CachedEntry {
    kind: MessageKind,
    content_len: usize,
    width: u16,
    lines: Vec<Line<'static>>,
}

/// Cheap discriminant for `ChatMessage`, used only to invalidate the cache
/// safely across `messages.insert(at, ...)` (peer messages can be inserted
/// before the streaming tail, shifting its index — see
/// `app/events.rs::StreamEvent::...` peer-message handling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MessageKind {
    User,
    Assistant,
    System,
}

fn message_kind_and_len(message: &ChatMessage) -> (MessageKind, usize) {
    match message {
        ChatMessage::User { content, .. } => (MessageKind::User, content.len()),
        ChatMessage::Assistant { content, .. } => (MessageKind::Assistant, content.len()),
        ChatMessage::System { content } => (MessageKind::System, content.len()),
    }
}
```

Add the cached entry point, calling the existing `build_all_lines` machinery per-message instead of only as one monolithic function. First, check whether `build_all_lines`'s per-message dispatch (the `match message { ... }` inside its `for` loop) can be called for ONE message at a time without duplicating logic — if `render_user_message`/`render_assistant_message`/`render_system_message` already take a single message's fields and push into a `&mut Vec<Line<'static>>` (confirmed true from this plan's research), reuse them directly:

```rust
/// Cached variant of [`build_all_lines`]. Produces identical output (see
/// `build_all_lines_cached_matches_uncached_output`); the only difference is
/// that unchanged messages skip re-formatting.
///
/// Takes `messages`/`verbose`/`spinner_frame` as separate parameters rather
/// than `&AppState` deliberately: the caller (`render_chat_area`) needs to
/// pass `&state.messages` (shared) alongside `&mut state.chat_line_cache`
/// (exclusive) in the same call. Rust's disjoint-field borrowing allows that
/// when the call site borrows fields directly, but NOT if this function took
/// `state: &AppState` as one opaque parameter — the compiler can't see
/// through that to know only `messages`/`verbose`/`spinner_frame` are read.
fn build_all_lines_cached(
    messages: &[ChatMessage],
    verbose: bool,
    spinner_frame: usize,
    width: u16,
    cache: &mut LineCache,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (idx, message) in messages.iter().enumerate() {
        let (kind, content_len) = message_kind_and_len(message);
        let is_streaming_now = matches!(message, ChatMessage::Assistant { is_streaming: true, .. });
        let hit = cache.entries.get(&idx).filter(|e| {
            e.kind == kind && e.content_len == content_len && e.width == width
        });
        let message_lines = if let Some(entry) = hit {
            entry.lines.clone()
        } else {
            let mut buf = Vec::new();
            match message {
                ChatMessage::User { content, timestamp } => {
                    render_user_message(content, timestamp, width, &mut buf);
                }
                ChatMessage::Assistant { content, tools, reasoning, is_streaming } => {
                    render_assistant_message(
                        content, tools, reasoning.as_deref(), *is_streaming,
                        verbose, spinner_frame, width, &mut buf,
                    );
                }
                ChatMessage::System { content } => {
                    render_system_message(content, width, &mut buf);
                }
            }
            // A streaming message's spinner/tool-block content can change
            // every tick without `content_len` changing (e.g. tool status),
            // so don't cache it — it would serve stale tool-block state.
            // Everything else (settled messages) is safe to cache.
            if !is_streaming_now {
                cache.entries.insert(idx, CachedEntry {
                    kind, content_len, width, lines: buf.clone(),
                });
            } else {
                cache.entries.remove(&idx);
            }
            buf
        };
        lines.extend(message_lines);
        lines.push(Line::default());
    }
    // Drop cache entries for indices beyond the current message count (a
    // conversation switch or `.clear()` shrinks the vec).
    cache.entries.retain(|idx, _| *idx < messages.len());
    lines
}
```

- [ ] **Step 5: Wire `render_chat_area` to use the cached path, threading the cache through `AppState`**

Read `interfaces/tui/src/tui/app/mod.rs` around the `AppState` struct definition (the `pub messages: Vec<ChatMessage>` field, found at line 637 in this plan's research) to find a suitable place to add a new field:

```rust
    /// Per-message rendered-line cache for the chat area — see
    /// `widgets::chat_area::LineCache`. Not part of any serialized/exported
    /// state; purely a render-time optimization.
    pub chat_line_cache: crate::tui::widgets::chat_area::LineCache,
```

Add it to `AppState::new(...)`'s constructor with `LineCache::default()`.

In `chat_area.rs`'s `render_chat_area`, change:

```rust
    let all_lines = build_all_lines(state, content_width);
```

to:

```rust
    let all_lines = build_all_lines_cached(
        &state.messages,
        state.verbose,
        state.spinner_frame,
        content_width,
        &mut state.chat_line_cache,
    );
```

This requires `render_chat_area` to take `state: &mut AppState` instead of `state: &AppState` (the body only needs `&mut` for the `&mut state.chat_line_cache` borrow above — every other read of `state` in `render_chat_area`, e.g. `state.focus`, stays a plain field read and compiles unchanged under `&mut AppState`). Check `render_chat_area`'s signature and all call sites (`render.rs`, likely) and update them accordingly; this is a mechanical `&` → `&mut` propagation, not a logic change.

Keep `build_all_lines` (the original, uncached function) in place, unchanged — it's still used by `build_all_lines_cached_matches_uncached_output` and by the existing tests that call it directly (`build_lines_with_system_message` etc., which don't need the cache).

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p aleph-tui --lib chat_area`
Expected: PASS, all tests (existing + 4 new).

- [ ] **Step 7: Run the full TUI test suite**

Run: `cargo test -p aleph-tui --lib`
Expected: PASS, no regressions from the `&AppState` → `&mut AppState` signature change.

- [ ] **Step 8: Lint**

Run: `cargo clippy -p aleph-tui -- -D warnings`

- [ ] **Step 9: Commit**

```bash
git add interfaces/tui/Cargo.toml interfaces/tui/src/tui/widgets/chat_area.rs interfaces/tui/src/tui/app/mod.rs interfaces/tui/src/tui/render.rs
git commit -m "perf: cache per-message rendered lines in the TUI chat area

build_all_lines_cached skips re-running markdown_to_lines for settled
messages whose (kind, content length, width) haven't changed, instead of
reformatting every message in history on every frame."
```

---

### Task 6: TUI — incremental tail rendering for the streaming message

**Files:**
- Modify: `interfaces/tui/src/tui/markdown.rs`
- Modify: `interfaces/tui/src/tui/widgets/chat_area.rs`

**Interfaces:**
- Consumes: `shared_ui_logic::markdown_stream::safe_freeze_offset` (Task 1/2), `LineCache` (Task 5).
- Produces: nothing consumed elsewhere.

- [ ] **Step 1: Write the failing test**

Add to `interfaces/tui/src/tui/markdown.rs`'s `#[cfg(test)] mod tests` (check the file for an existing test module first; create one with `use super::*;` if none exists):

```rust
    #[test]
    fn incremental_and_full_conversion_produce_identical_lines() {
        let text = "line one\n```rust\nfn f() {}\n```\nline two\n";
        let full = markdown_to_lines(text, 80);

        let mut cache: Option<(usize, Vec<Line<'static>>)> = None;
        let (_offset, incremental) = markdown_to_lines_incremental(text, 80, &mut cache);
        assert_eq!(full, incremental);
    }

    #[test]
    fn incremental_conversion_reuses_the_cache_on_a_second_call_with_more_text() {
        let mut cache: Option<(usize, Vec<Line<'static>>)> = None;
        let first_text = "line one\n";
        let (offset1, lines1) = markdown_to_lines_incremental(first_text, 80, &mut cache);
        assert!(offset1 > 0);
        assert_eq!(cache.as_ref().map(|(o, _)| *o), Some(offset1));

        let grown_text = "line one\nline two\n";
        let (offset2, lines2) = markdown_to_lines_incremental(grown_text, 80, &mut cache);
        assert!(offset2 >= offset1);
        assert_eq!(lines2, markdown_to_lines(grown_text, 80));
        let _ = lines1; // only asserted for the offset progression above
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p aleph-tui --lib markdown::tests::incremental -- --nocapture`
Expected: FAIL — `markdown_to_lines_incremental` doesn't exist.

- [ ] **Step 3: Implement `markdown_to_lines_incremental`**

In `interfaces/tui/src/tui/markdown.rs`, add:

```rust
/// Incremental variant of [`markdown_to_lines`] for a still-growing message.
///
/// `cache` holds `(safe_offset, lines_for_that_prefix)` from the previous
/// call. Only the text from `safe_offset` to the new
/// `shared_ui_logic::markdown_stream::safe_freeze_offset` boundary is
/// re-converted; the rest of the cached `Vec<Line>` is reused as-is. Falls
/// back to a full re-run of [`markdown_to_lines`] on the very first call
/// (`cache == None`) and whenever no further safe progress exists (the
/// cached lines are still returned unchanged in that case).
///
/// Returns `(new_safe_offset, full_lines_for_the_whole_text)` — the second
/// element is what callers render; the first is what they should pass back
/// in `cache` (already stored into `*cache` by this function) on the next
/// call.
pub fn markdown_to_lines_incremental(
    text: &str,
    width: u16,
    cache: &mut Option<(usize, Vec<Line<'static>>)>,
) -> (usize, Vec<Line<'static>>) {
    let (prev_offset, prev_lines) = cache.clone().unwrap_or((0, Vec::new()));
    match shared_ui_logic::markdown_stream::safe_freeze_offset(text, prev_offset) {
        Some(new_offset) if new_offset > prev_offset => {
            // The safe prefix grew. Re-run full conversion ONLY on the safe
            // prefix (cheap relative to the whole growing text as long as
            // fences close reasonably often) and append the tail from
            // markdown_to_lines run on just the remainder, matching
            // markdown_to_lines's own fence-tracking semantics (it always
            // starts a fresh scan at `in_code_block = false`, which is valid
            // exactly at a safe-offset boundary by construction).
            let mut lines = markdown_to_lines(&text[..new_offset], width);
            let tail = &text[new_offset..];
            if !tail.is_empty() {
                lines.extend(markdown_to_lines(tail, width));
            }
            *cache = Some((new_offset, lines.clone()));
            (new_offset, lines)
        }
        _ => {
            // No new safe progress: reformat only the tail past the cached
            // safe offset and append it to the cached prefix lines.
            let mut lines = prev_lines.clone();
            let tail = &text[prev_offset..];
            if !tail.is_empty() {
                lines.extend(markdown_to_lines(tail, width));
            }
            *cache = Some((prev_offset, prev_lines));
            (prev_offset, lines)
        }
    }
}
```

**Note on the cost model**: unlike Task 3's Panel version (which only re-processes the newly-safe delta and appends pre-rendered HTML), this TUI version re-runs `markdown_to_lines` on the whole safe prefix `text[..new_offset]` when the boundary advances, because `markdown_to_lines` returns `Vec<Line<'static>>` with wrapped/styled spans that aren't trivially concatenable the way HTML strings are (a `Line` wrapped at a width boundary can differ depending on what came before it in the same paragraph). This still avoids reprocessing whenever the boundary DOESN'T advance (the common case — most ticks arrive between safe-offset advances), and the tail-only reprocessing (`text[new_offset..]` / `text[prev_offset..]`) is always bounded by "how far behind the safe boundary trails," not by total message length. If profiling after this ships shows the prefix reformat is still too costly for very long streaming messages, that's a Phase 2 candidate (see spec §7) — not attempted here (YAGNI).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p aleph-tui --lib markdown`
Expected: PASS.

- [ ] **Step 5: Wire the incremental path into `chat_area.rs` for the streaming message**

In `chat_area.rs`'s `build_all_lines_cached` (Task 5), the streaming-message branch currently calls `render_assistant_message` fresh every time (never cached). Change `render_assistant_message`'s content-rendering line (inside `chat_area.rs`, was `markdown_to_lines(content, content_width)` at line 209 for the assistant path) to use the incremental path when streaming. This requires threading a per-message incremental cache alongside `LineCache`; extend `LineCache` to its full new shape (adds two fields to the one Task 5 created):

```rust
#[derive(Default)]
pub struct LineCache {
    entries: HashMap<usize, CachedEntry>,
    streaming_markdown_cache: Option<(usize, Vec<Line<'static>>)>,
    streaming_message_idx: Option<usize>,
}
```

Reset `streaming_markdown_cache` to `None` whenever the streaming message's index changes (a new message starts streaming) — add this check at the top of `build_all_lines_cached`:

```rust
    let streaming_idx = state.messages.iter().position(
        |m| matches!(m, ChatMessage::Assistant { is_streaming: true, .. })
    );
    if cache.streaming_message_idx != streaming_idx {
        cache.streaming_markdown_cache = None;
        cache.streaming_message_idx = streaming_idx;
    }
```

(Add `streaming_message_idx: Option<usize>` to `LineCache` alongside the field above, defaulting to `None`.)

Then, in the `ChatMessage::Assistant` branch inside `build_all_lines_cached`'s per-message dispatch, when `*is_streaming` is true, use the incremental path for the content portion instead of the plain `markdown_to_lines` call. Add a new parameter to `render_assistant_message`'s signature (verified at `chat_area.rs:140-149`):

```rust
#[allow(clippy::too_many_arguments)]
fn render_assistant_message(
    content: &str,
    tools: &[crate::tui::app::ToolExecution],
    reasoning: Option<&str>,
    is_streaming: bool,
    verbose: bool,
    spinner_frame: usize,
    width: u16,
    lines: &mut Vec<Line<'static>>,
    streaming_cache: Option<&mut Option<(usize, Vec<Line<'static>>)>>,
) {
```

Change the content-rendering block inside it (was, unconditionally, `let md_lines = markdown_to_lines(content, content_width); ...` around line 209) to:

```rust
    if !content.is_empty() {
        let content_width = width.saturating_sub(2);
        let md_lines = match streaming_cache {
            Some(cache) => {
                let (_offset, lines) = markdown_to_lines_incremental(content, content_width, cache);
                lines
            }
            None => markdown_to_lines(content, content_width),
        };
        for md_line in md_lines {
            let mut spans = vec![Span::styled("\u{2503} ", prefix_style)];
            spans.extend(md_line.spans);
            lines.push(Line::from(spans));
        }
    }
```

Update every existing call site of `render_assistant_message` to pass `None` for the new last argument, with one exception. In `build_all_lines` (the original, uncached function — still used directly by several existing tests) and any test in `chat_area.rs`'s `#[cfg(test)] mod tests` that calls it directly: pass `None`.

In `build_all_lines_cached`'s `ChatMessage::Assistant` arm (from Task 5), the cache slot must only be handed to the message that is ACTUALLY streaming — passing it unconditionally would let an unrelated non-streaming message's render call clobber the one streaming message's incremental cache. Change that arm to:

```rust
                ChatMessage::Assistant { content, tools, reasoning, is_streaming } => {
                    render_assistant_message(
                        content, tools, reasoning.as_deref(), *is_streaming,
                        verbose, spinner_frame, width, &mut buf,
                        if *is_streaming { Some(&mut cache.streaming_markdown_cache) } else { None },
                    );
                }
```

(This replaces the 8-argument call Task 5 wrote in this same arm with the 9-argument form above — the first 8 arguments are unchanged; `verbose`/`spinner_frame` are `build_all_lines_cached`'s own parameters, not `state.verbose`/`state.spinner_frame` — see Task 5's note on why this function takes them separately rather than a whole `&AppState`.)

- [ ] **Step 6: Run the full TUI test suite**

Run: `cargo test -p aleph-tui --lib`
Expected: PASS.

- [ ] **Step 7: Lint**

Run: `cargo clippy -p aleph-tui -- -D warnings`

- [ ] **Step 8: Manual smoke check**

Run the TUI against a real provider (existing pty-driven QA recipe), send a message that streams a long response with a code block, confirm the rendered text and code fence look identical to before this change (no truncation, no duplicated lines, no missing fence styling).

- [ ] **Step 9: Commit**

```bash
git add interfaces/tui/src/tui/markdown.rs interfaces/tui/src/tui/widgets/chat_area.rs
git commit -m "perf: incremental tail-only markdown conversion for the streaming TUI message"
```

---

### Task 7: TUI — skip `terminal.draw()` on ticks that change nothing

**Files:**
- Modify: `interfaces/tui/src/tui/mod.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks (independent; benefits compound with Tasks 5/6 but doesn't require them).
- Produces: nothing consumed elsewhere — this is the last task.

- [ ] **Step 1: Write the failing test**

`main_loop` is an integration-level async function wired to real terminal/channel I/O, not easily unit-tested in isolation. Extract the pure decision — "given this action and the connection-state edge, should the next iteration redraw?" — into a standalone testable function first.

Add to `interfaces/tui/src/tui/mod.rs` (or a new `#[cfg(test)] mod tests` if `mod.rs` doesn't have one — check first):

```rust
    #[test]
    fn tick_with_no_active_run_and_no_connection_change_does_not_redraw() {
        assert!(!should_redraw_after_tick(false, false));
    }

    #[test]
    fn tick_with_an_active_run_redraws_to_animate_the_spinner() {
        assert!(should_redraw_after_tick(true, false));
    }

    #[test]
    fn tick_with_a_connection_state_change_redraws_even_when_idle() {
        assert!(should_redraw_after_tick(false, true));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p aleph-tui --lib should_redraw_after_tick -- --nocapture`
Expected: FAIL — function doesn't exist.

- [ ] **Step 3: Implement `should_redraw_after_tick` and wire the dirty flag into `main_loop`**

Add near the top of `mod.rs` (outside `main_loop`, so it's testable without the async machinery):

```rust
/// Whether a pure `Action::Tick` (no gateway/terminal event) needs a redraw.
///
/// A tick that only bumped the spinner counter with nothing on screen
/// depending on it (`has_active_run == false`) changes nothing visible and
/// can skip the draw entirely — the per-message line cache (see
/// `widgets::chat_area::LineCache`) already makes an idle draw cheap, but
/// cheap is not free, and a genuinely idle terminal has no reason to redraw
/// 20 times a second. `connection_state_changed` covers the one other thing
/// a pure tick can affect: the status dot flips on the disconnect/reconnect
/// edge (see the tick handler's own comment on why the edge, not the level,
/// matters).
fn should_redraw_after_tick(has_active_run: bool, connection_state_changed: bool) -> bool {
    has_active_run || connection_state_changed
}
```

In `main_loop`, replace the unconditional draw (`terminal.draw(|f| render::render(f, state, textarea))?;` at the top of `loop { ... }`) with a flag that starts `true` (Global Constraint: first frame always draws):

```rust
    let mut needs_redraw = true;
    loop {
        let mut reconnect_outcome: Option<CliResult<()>> = None;
        if needs_redraw {
            terminal.draw(|f| render::render(f, state, textarea))?;
            needs_redraw = false;
        }

        let action = tokio::select! { /* unchanged */ };
```

After the `match action { ... }` block handles `Action::Tick` (the existing body — spinner bump, connection check, approval poll — stays exactly as-is), capture whether the connection state changed and set `needs_redraw` accordingly. Change the `Action::Tick` arm's body from:

```rust
            Action::Tick => {
                state.spinner_frame = state.spinner_frame.wrapping_add(1);
                let live = client.is_connected();
                if live {
                    state.is_connected = true;
                } else if state.is_connected {
                    state.on_disconnected();
                    state.add_system_message(
                        "Connection lost — reconnecting in the background.".to_string(),
                    );
                    backoff = Duration::ZERO;
                    reported_reconnect_failure = false;
                    reconnecting = Some(reconnect_after(client, config, backoff));
                }
                if state.current_run.is_some() && state.spinner_frame.is_multiple_of(20) {
                    approval::poll_approvals(state, client).await;
                }
            }
```

to (only the connection-tracking and the final line change; everything else stays identical):

```rust
            Action::Tick => {
                state.spinner_frame = state.spinner_frame.wrapping_add(1);
                let was_connected = state.is_connected;
                let live = client.is_connected();
                if live {
                    state.is_connected = true;
                } else if state.is_connected {
                    state.on_disconnected();
                    state.add_system_message(
                        "Connection lost — reconnecting in the background.".to_string(),
                    );
                    backoff = Duration::ZERO;
                    reported_reconnect_failure = false;
                    reconnecting = Some(reconnect_after(client, config, backoff));
                }
                if state.current_run.is_some() && state.spinner_frame.is_multiple_of(20) {
                    approval::poll_approvals(state, client).await;
                }
                needs_redraw = should_redraw_after_tick(
                    state.current_run.is_some(),
                    state.is_connected != was_connected,
                );
            }
```

For every OTHER `Action::*` arm (`SendMessage`, and all the rest already in the `match`), and for the terminal-event and gateway-event branches of the `select!` (which produce actions other than `Tick` via `keys::handle_terminal_event`/`state.handle_gateway_event`), set `needs_redraw = true` unconditionally — these always represent a real state change worth showing. The simplest correct way to guarantee this without touching every existing arm: set `needs_redraw = true` right after the `match action { ... }` block for every action EXCEPT `Tick` (which sets it itself above) and `None`/`Quit` (which don't need a redraw — `Quit` exits the loop, `None` means nothing happened). Add this after the full `match action { ... }` block:

```rust
        if !matches!(action, Action::Tick | Action::None | Action::Quit) {
            needs_redraw = true;
        }
```

Also set `needs_redraw = true` whenever `reconnect_outcome` was handled (a reconnect completing or failing always changes visible state — the status dot and/or a system message get added):

```rust
        if let Some(outcome) = reconnect_outcome {
            reconnecting = None;
            needs_redraw = true;
            match outcome { /* unchanged */ }
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p aleph-tui --lib should_redraw_after_tick`
Expected: PASS.

- [ ] **Step 5: Run the full TUI test suite**

Run: `cargo test -p aleph-tui --lib`
Expected: PASS, no regressions.

- [ ] **Step 6: Lint**

Run: `cargo clippy -p aleph-tui -- -D warnings`

- [ ] **Step 7: Manual smoke check (this is the one most likely to hide a regression in a unit test)**

Run the TUI, and specifically verify: (a) the spinner visibly animates while waiting for the first token of a response (this is the scenario `should_redraw_after_tick`'s `has_active_run` branch exists for — a pure tick with no gateway event yet), (b) typing in the composer is immediately responsive (terminal events always redraw), (c) the connection status dot still flips promptly on a simulated disconnect/reconnect, (d) leaving the TUI idle for 30+ seconds does not visibly break anything when a new message finally arrives.

- [ ] **Step 8: Commit**

```bash
git add interfaces/tui/src/tui/mod.rs
git commit -m "perf: skip terminal.draw() on idle ticks that change nothing visible

A pure Action::Tick with no active run and no connection-state change now
skips the redraw instead of unconditionally rebuilding and diffing the
whole frame 20x/sec. Every other action (keystroke, gateway event,
reconnect outcome) still redraws immediately — first-frame and
in-progress-run responsiveness are unaffected."
```

---

## Final Verification (after all 7 tasks)

```bash
cargo check -p alephcore
cargo check -p aleph-tui
cargo check -p aleph-panel --target wasm32-unknown-unknown
cargo clippy -p aleph-tui -p shared-ui-logic -- -D warnings
cargo test -p shared-ui-logic --lib
cargo test -p aleph-tui --lib
cargo test -p aleph-panel --lib
just wasm
```

Then repeat the manual QA pass from spec §8: a real provider-driven multi-token streaming run in both Panel (Puppeteer/Chrome-MCP QA rig) and TUI (pty-driven QA recipe), specifically re-checking the typewriter reveal animation and code-fence rendering end to end, not just per-task in isolation — some regressions (e.g. a fence boundary interacting with the row-key stabilization AND the incremental cache at once) can only show up when Tasks 3, 4, 5, and 6 are all present together.
