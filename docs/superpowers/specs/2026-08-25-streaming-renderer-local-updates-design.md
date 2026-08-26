# Streaming Renderer Local-Update Refactor — Design Spec

**Date:** 2026-08-25
**Scope:** Panel streaming message rendering (`interfaces/webchat/`) + TUI streaming
message rendering (`interfaces/tui/`)
**Branch:** `streaming-renderer-local-updates-2026-08-25` (worktree, per explicit
user instruction for this task — overrides the project's default single-branch-on-
main convention for this occasion only)
**Status:** Approved direction (Approach A), pending final spec review

## 1. Background

User-reported symptom: streaming responses feel janky (visible stutter on long
responses) **and** burn more CPU/battery than they should, roughly equally, in
both Panel and TUI. Neither symptom has a prior FEATURE_LOCATOR entry — this is
new ground, not a regression of previously-hardened behavior.

Research covered three angles in parallel: (1) Panel's actual streaming render
pipeline, (2) TUI's actual streaming render pipeline, (3) two reference projects —
`pi` (TS, custom terminal renderer with per-leaf memoization + debounced render
scheduling) and `codex` (Rust/ratatui, the more directly comparable reference —
splits "settled" history into the terminal's own scrollback and a newline-gated
incremental markdown commit for the live tail).

Initial research reports were verified against actual source (not taken on
faith) — see §2 for corrections that changed the design's focus.

## 2. Gap Analysis (verified)

| Dimension | Aleph Panel (verified) | Aleph TUI (verified) | pi | codex |
|---|---|---|---|---|
| Render trigger | Every WS token → `messages.set()` → `rows` Memo re-walks full list | Unconditional `terminal.draw()` every loop iteration incl. pure 50ms spinner ticks (`tui/mod.rs:308`) | Debounced (`requestRender` + `process.nextTick`) | Adaptive Smooth/CatchUp scheduler decouples token arrival from render rate |
| List/row identity | `<For>` keyed on `id + content.len()` (`timeline.rs:232-246`) → **key changes every token → whole message bubble subtree unmounts/remounts** | N/A, immediate-mode | Per-leaf memoized on `(text, width)` | Finalized rows physically exit the render loop (written once to real terminal scrollback) |
| Per-token content cost | **Already mitigated for the streaming case**: `render_streaming` (`components/markdown.rs:237`) is a cheap O(n) escape+fence-scan, no pulldown-cmark, no syntect. Full parse+syntect (`render_markdown`) only runs once, on completion. Remaining cost: `render_streaming` re-scans the **entire revealed-so-far substring** every ~33ms tick — no prefix cache. | `build_all_lines` (`chat_area.rs:67`) reformats **every message in history**, not just the streaming one, every frame — zero caching (confirmed: no `cache`/`memo`/`dirty` hits in the tree) | Leaf cache skips re-tokenize/re-wrap for unchanged `(text,width)` | `MarkdownStreamCollector` commits only up to the last completed `\n`; tail-only reparse |
| Reveal-position durability | Already solved: `TypewriterClock` (context, keyed by `message_id`) survives the per-token remount by design — the remount itself, not content correctness, is the remaining problem | N/A | — | — |
| Viewport | N/A (DOM) | Full transcript formatted every frame, then sliced — off-screen work wasted | Layout tree cheap, content cached | Settled history never re-touched by the render loop at all |
| Dirty tracking | None (Leptos fine-grained reactivity does this implicitly *if* the code lets it — see §3) | None — no `dirty`/`version`/`generation` field anywhere in `AppState` | Throttle substitutes | `active_cell_transcript_key()` |
| Data-model layer | `append_chunk` mutates the message signal directly (correct) | Already correct: `app/trace.rs` is append-only (`push_str`, not rebuild) | — | — |

**Correction from initial research pass**: the first-pass report characterized
Panel's streaming path as "full markdown reparse every token." Verified against
`components/markdown.rs`, this is **not accurate** — the code already defers full
parse+syntax-highlighting to completion. The real, verified costs are narrower:
(a) DOM churn from unstable `<For>` keys, (b) O(revealed-length) rescanning with
no prefix cache in the typewriter tick, (c) full message-list refold in
`build_rows` on every token (smaller, secondary). Design below targets (a) and
(b) as primary; (c) is explicitly deferred (§7).

## 3. Design Decisions

### Decision 1: Approach A (boundary-gated incremental render), not B or C

Three approaches were presented; user confirmed A.

- **A — chosen.** Stabilize row identity (stop the DOM remount) + cache the
  already-processed prefix so per-tick work is O(new content) not
  O(total-revealed). Narrow blast radius, reuses `shared-ui-logic` (already a
  workspace member, already consumed by Panel with `leptos`/`wasm` features,
  not yet a TUI dependency).
- **B — deferred, not built now.** codex's most radical technique (write
  settled TUI content directly into the terminal's real scrollback via raw
  crossterm scroll-region escapes; Panel equivalent: fully non-reactive frozen
  DOM for settled messages). Strictly more powerful for very long transcripts,
  but real cross-platform terminal risk (this codebase has documented Windows
  terminal/DPI fragility elsewhere) and requires a "thaw" escape hatch for any
  future feature that mutates settled messages (locale re-render, in-transcript
  search highlighting). Revisit only if Phase 1 measurements on long real
  sessions show remaining cost still matters.
- **C — rejected.** Dirty-flag + coarse hash-memo only, no shared crate, no
  tail-parsing. Would fix TUI's full-history-rewalk cost but leaves the
  actively-streaming message's own O(revealed-length) rescan untouched — that
  is very likely the dominant contributor to the *jank* symptom specifically,
  which the user said matters equally to the resource-cost symptom. Rejected
  because it under-delivers on half the stated problem.

### Decision 2: Shared logic lives in `shared/ui_logic`, not a new crate

`shared-ui-logic` already exists, is a workspace member, and its non-Leptos
modules (`state/chat_scroll.rs`, `state/composer_queue.rs`) already prove the
crate is structured to hold pure logic usable outside WASM. Adding
`markdown_stream` here is "connect first" — TUI adds it as a dependency with
`default-features = false` (skips `leptos`/`wasm-bindgen`/`web-sys`), Panel
already pulls the crate in with those features on. No new workspace member.

### Decision 3: Row model changes from owned snapshot to id + reactive lookup

`TimelineRow::Message`/`Narration` currently carry an owned `ChatMessage`
snapshot (`timeline.rs:33,37`). Stabilizing the `<For>` key without this change
would freeze the rendered content at mount time — Leptos only re-runs a `<For>`
child's closure when its key changes, so a stable key with a captured-by-value
snapshot would silently stop updating. The row must instead carry just `id`,
with the actual `ChatMessage` fetched via a per-row `Memo` that reads the
`messages` signal. `ChatMessage` already derives `PartialEq`
(`state/mod.rs:214`), so Leptos's Memo machinery already skips downstream
notification when the fetched value is unchanged — unaffected rows pay a cheap
equality check, not a rebuild.

### Decision 4: `build_rows` full-refold is out of scope for Phase 1

`build_rows` (`timeline.rs:65`) folds the entire message list on every
`messages` signal change — O(message count), not O(character count). Given
typical conversation lengths (tens to low hundreds of messages), this is a much
smaller cost than (a) DOM churn or (b) per-tick rescanning. Splitting it into a
"stable prefix / live tail" structure adds real complexity (cache invalidation
when a *non-tail* message changes, e.g. a tool result arriving out of order) for
a cost that hasn't been shown to matter yet. Deferred — revisit with real
profiling data after Phase 1 ships.

### Decision 5: TUI draw coalescing must not delay the first frame of a run

Debouncing `terminal.draw()` risks the user perceiving added latency at the
start of a response if the very first frame is held back. The coalescing window
only applies to *subsequent* bursts within ~16-33ms of a draw that already
happened; a run's first content-bearing event always draws immediately.

## 4. Architecture

```
shared/ui_logic/src/
  markdown_stream/
    mod.rs        # StreamBoundary: given accumulated text + last-known-safe
                   # offset, returns the new safe-to-freeze offset, or None
                   # (caller must fully reprocess this update — correctness
                   # never regresses, only the perf win is forfeited)
    boundary.rs    # pure text scanning: fence open/close tracking, blank-line
                   # block-close detection, reference-link-definition detection
                   # (matches codex's own documented unsafe-case fallback)
```

Both Panel and TUI keep their own renderers (Panel: pulldown-cmark → HTML; TUI:
hand-rolled `markdown_to_lines` → ratatui `Line`s) — only the "how far is it
safe to treat as frozen" decision is shared, not the rendering.

### Panel changes

| File | Change |
|---|---|
| `interfaces/webchat/src/platform/wide/views/chat/timeline.rs` | `row_key` (L232-246): drop `content.len()` from the key for `Message`/`Narration` rows. `TimelineRow::Message`/`Narration` carry `id: String` instead of an owned `ChatMessage`. |
| `interfaces/webchat/src/platform/wide/views/chat/messages.rs` | Row rendering closure: replace captured owned message with `Memo::new(move \|_\| chat.messages.with(\|m\| m.iter().find(\|x\| x.id == id).cloned()))`. Remove the `!is_streaming` gate on the entrance animation (`L892-899`) — no longer needed once bubbles stop remounting per token; this is a direct cleanup enabled by the fix, not a separate task. |
| `interfaces/webchat/src/components/markdown.rs` | `TypewriterRenderer`: add `(stable_html: String, stable_revealed: usize)` per message, advance via `shared_ui_logic::markdown_stream::advance`, only re-run `render_streaming` on `content[stable_revealed..revealed]`, append to `stable_html`. Falls back to full `render_streaming(&shown)` when the boundary detector returns `None`. |

### TUI changes

| File | Change |
|---|---|
| `interfaces/tui/Cargo.toml` | Add `shared-ui-logic = { path = "../../shared/ui_logic", default-features = false }` |
| `interfaces/tui/src/tui/widgets/chat_area.rs` | `build_all_lines`: per-message cache `HashMap<MessageId, (usize /* content.len() at cache time */, u16 /* width */, Vec<Line<'static>>)>`. `content.len()` is a correct (not just cheap) change-detector here because streaming content is append-only (`trace.rs::push_str`) — length is monotonically non-decreasing while a message streams and constant once settled, so a length mismatch always means real new content, never a false miss. The currently-streaming message always misses this cache by construction (its length changes every token) and is handled by the separate incremental tail path below, not by this cache. |
| `interfaces/tui/src/tui/markdown.rs` | `markdown_to_lines`: same stable-prefix/live-tail treatment via `shared_ui_logic::markdown_stream`, caching converted `Line`s for the frozen prefix. |
| `interfaces/tui/src/tui/mod.rs` | `main_loop` (L308): replace unconditional `terminal.draw()` with a dirty flag + ~16-33ms coalescing window. First frame of a run always draws immediately (Decision 5). |

## 5. Data Flow (one streaming token, before → after)

**Panel, before**: WS delta → `append_chunk` → `messages.set()` → `rows` Memo
re-walks all messages → `<For>` sees changed key on the streaming row →
unmounts+remounts the whole bubble subtree → fresh `TypewriterRenderer`
instance (content correctness preserved only because `TypewriterClock` state
lives outside the component) → `render_streaming` re-scans the entire revealed
substring.

**Panel, after**: WS delta → `append_chunk` → `messages.set()` → `rows` Memo
re-walks all messages (unchanged cost, Decision 4) → `<For>` sees the same key
for the streaming row, no mount/unmount → the row's `Memo` detects the changed
`ChatMessage`, downstream signal updates → `TypewriterRenderer`'s persistent
instance advances `stable_revealed`, reprocesses only the new tail.

**TUI, before**: any event/tick → unconditional `terminal.draw()` →
`build_all_lines` walks every message, calls `markdown_to_lines` on all of them
from scratch.

**TUI, after**: gateway event → mark dirty (may coalesce) → `terminal.draw()`
(throttled, first-frame exempt) → `build_all_lines` hits cache for every
message except the streaming one → that one advances its own stable/tail
boundary.

## 6. Error Handling / Edge Cases

- **Unsafe-to-freeze tail** (mid-fence boundary, dangling reference-link
  definition): boundary detector returns `None`; caller does a full reprocess
  for that update. Correctness never regresses — only that update's perf win is
  forfeited. Mirrors codex's own documented escape hatch.
- **Message id changes mid-stream**: should not happen (`id` stamped once in
  `start_assistant_message`); if it ever does, the per-row `Memo` lookup
  returns `None` and the row renders a placeholder rather than panicking.
- **TUI cache invalidation**: keyed on `(content.len(), width)` — a terminal
  resize invalidates every cached entry (width changed); content mutation
  invalidates only that message's entry (length changed).
- **TUI draw coalescing**: must not eat the first frame of a run (Decision 5).
- **Skip-to-end click** (existing `TypewriterClock::skip` feature): must
  continue to work when the renderer holds a `stable_html` prefix cache — skip
  jumps `revealed` to `total`; the incremental renderer must process the
  remaining `[stable_revealed..total]` range in one shot, not require N ticks.

## 7. Out of Scope (deliberate, not oversights)

- **Approach B** (codex-style structural freeze into real TUI scrollback /
  fully non-reactive Panel DOM for settled messages) — deferred per Decision 1,
  documented here so it isn't silently forgotten and isn't relitigated without
  new evidence.
- **`build_rows` stable-prefix/live-tail split** — deferred per Decision 4.
- **syntect caching/optimization** — not needed; syntect already only runs
  once per message at completion, never per-token. No change proposed here.
- **New workspace crate** — rejected; `shared-ui-logic` already fits.
- **Rewriting Panel's component library or TUI's widget system** — not
  required; both keep their existing rendering, only the boundary/caching layer
  is added.
- **pi's/codex's actual widget/component systems** — not adopted; only the
  "where to put the freeze boundary and the commit scheduler" *pattern* is
  transplanted, per the reference research synthesis.

## 8. Testing Plan

- Pure logic: `shared_ui_logic::markdown_stream` boundary detection — host unit
  tests, no Leptos/ratatui/WASM needed.
- Panel: extend existing headless `messages.rs`/`events.rs` token-simulation
  test harness to assert the streaming row's key stays stable across a run and
  only changes on structural transitions (new test, mirrors existing
  `row_key_narration_changes_on_content_growth` style in `timeline.rs`).
- TUI: extend `cargo test -p aleph-tui --lib` coverage in `chat_area.rs` with
  cache-hit/cache-miss assertions on `build_all_lines`.
- Manual verification (required — headless tests don't cover visual
  regression): real provider-driven multi-token streaming run in both Panel
  (existing Puppeteer/Chrome-MCP QA rig) and TUI (existing pty-driven QA
  recipe), watching specifically for the typewriter reveal animation and code
  fence rendering, since those are the highest-risk-of-visible-breakage pieces.

## 9. Verification Commands

```bash
# Compile
cargo check -p alephcore
cargo check -p aleph-tui
cargo check -p aleph-panel --target wasm32-unknown-unknown

# Lint
cargo clippy -p aleph-tui -- -D warnings
cargo clippy -p shared-ui-logic -- -D warnings

# Tests
cargo test -p shared-ui-logic --lib
cargo test -p aleph-tui --lib
cargo test -p aleph-panel --lib   # NOT `cargo check` — this crate's #[cfg(test)]
                                   # modules are invisible to check (project
                                   # CLAUDE.md §10 caveat)

# Full wasm build (debug reads from disk; release actually re-embeds)
just wasm
```

## 10. Rollback

Worktree branch, not on `main`. If the manual QA pass surfaces visible
regressions in typewriter reveal or code-fence rendering that aren't
resolvable quickly, the branch is simply not merged — no rollback machinery
needed since nothing lands on `main` until this is verified end-to-end.
