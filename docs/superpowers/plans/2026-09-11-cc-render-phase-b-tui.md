# CC-Render Phase B — TUI (`interfaces/tui/`)

**Spec:** [`docs/superpowers/specs/2026-09-06-cc-style-transcript-rendering-design.md`](../specs/2026-09-06-cc-style-transcript-rendering-design.md) §6
**Branch:** `worktree-cc-render-r1`, fast-forwarded to `main` (`6eaa64b94`) on 2026-09-11 — Phase A is already **merged** (`fbfbc2090`), so this phase builds on current main rather than the old branch tip.
**Depends on:** `shared-ui-logic::transcript` (Phase A, zero consumers until this phase).

---

## 0. What changed since the spec was written

Three facts re-measured on 2026-09-11, because the spec's §6 assumed the branch had not merged:

1. **Phase A is in `main`.** `caab8bb8a` is an ancestor of `main`; `git diff HEAD main` over every Phase A path is empty. Phase B commits go on top of current main.
2. **`shared-ui-logic` is already a dependency of `aleph-tui`** (`interfaces/tui/Cargo.toml`), and `transcript` is already re-exported from its `lib.rs`. No wiring task is needed — the tree simply has no caller yet.
3. **`aleph-tui` must not depend on `alephcore`** (stated in its own `Cargo.toml`). Everything this phase consumes must come through `aleph-protocol` or `shared-ui-logic`. This kills any temptation to reach for a server-side helper.

## 1. The shared surface this phase consumes

Already built and tested in Phase A; this phase adds the first caller for each.

| Module | What B uses |
|---|---|
| `view_model` | `TranscriptEntry` (7 variants), `ToolRow{new,start,finish,settle_resumed,is_read_only,is_terminal}`, `RowStatus`, `RowBody`, `ToolGroup::headline` |
| `summarize` | `summarize(tool,&args) -> CallSummary{display_name,args_text}` |
| `fold` | `fold(lines,width,policy) -> Folded{rows,hidden_rows,hidden_lines,truncated}`, `FoldPolicy::for_display_name`, `wrap_physical` |
| `group` | `group_entries` |
| `diff_view` | `diff_rows`, `stats_label`, `DiffRow`, `Span`, `COLLAPSED_DIFF_ROWS` |
| `md_enhance` | `enhance -> Enhanced{blocks}`, `Block::{Markdown,Admonition,Mermaid}`, `find_path_refs` |
| `affordance` | `spinner_frame(now_ms)`, `verb(locale,seed)`, `worked_for`, `fmt_duration_ms`, `expand_hint(Modality,hidden)` |
| `context` | `reconcile(&ContextBreakdown) -> ContextRows` |
| `turn_summary` | `summarize_turn`, `turn_summary_text` |
| `theme_tokens` | `SemanticColor` (26 roles), `ALL_SEMANTIC_COLORS`, `mix_rgb`, `DIFF_ROW_MIX`, `DIFF_EMPHASIS_MIX` |

## 1b. Status (2026-09-11)

**B1, B2, B3 landed** — `6594bfca6` (shared `at_ms`) + `071fac0f0` (the TUI).
340 `aleph-tui` tests green, zero warnings; `aleph-cli` and `aleph-panel`
re-verified because the shared type moved.

Four things came out different from the plan below, and each is recorded where
it happened rather than only here:

1. **B1 and B2/B3 are one commit, not three.** `theme.rs`'s `/theme` arm lives
   in `commands.rs`, which is also where the history loader and the turn-undo
   moved; `app/mod.rs` holds both the palette invalidation and the model. A
   split by file would have produced commits that do not compile alone, which
   is worse than one commit that does.
2. **Grouping is deferred.** `group_entries` takes `Vec<TranscriptEntry>` by
   value, so calling it per frame reintroduces the O(transcript) deep copy
   `build_visible_lines` was written to remove. Reconciling the two — an
   index-range grouping, or a grouped list cached against a revision counter —
   is its own task, and doing it badly would undo a measured optimisation.
3. **The per-message `┃ Aleph` label is gone.** Spec §6 makes the identity a
   one-time header entry; repeating a label above every fragment of an
   interleaved turn is noise. The `┃ ` bar and the colour still mark it.
4. **`/theme` persists through `ALEPH_TUI_THEME`, not a settings file.** R4:
   an interface does not persist, and this crate has no config of its own. A
   terminal's colour preference is the same kind of fact as `COLORTERM`.

A correction to the spec, measured while writing B3's tests: §5's summary
table writes `shell→Bash`. There is no `shell` tool — the real name is `bash`
(`src/builtin_tools/bash_exec.rs`), and Phase A's `DISPLAY_NAMES` already keys
on it correctly. The spec line is the wrong one.

And a correction to my own work, made before this was committed: I had padded
the status glyph into a two-column cell on the claim that `⏺` (U+23FA) carries
`Emoji_Presentation` and therefore measures two columns. Measured, it does
not — `unicode-width` reports **one** column for it, the same as a braille
spinner frame, so the padding was solving nothing and shifted every header to
`⏺  Read(…)`. Worse, padding could not have solved it even had the premise
held: `ratatui` budgets cells with the same `unicode-width` the padding is
computed from, so a glyph the *terminal* paints wider stays misaligned however
many spaces follow. What survives is the invariant — every glyph that can open
a row is one column — asserted in `tool_row.rs` against `status_glyph`'s own
returns plus every spinner frame, and mutation-verified red with `🔴`. The
residual risk (a terminal that disagrees with `unicode-width` about `⏺`) is
invisible to every test in this repo and stays on the real-machine list below.

## 2. Tasks

Ordered so the tree compiles after each and every commit is independently reviewable.

### B1 — Theme: one role table, three presets, `/theme` ✅

`interfaces/tui/src/tui/theme.rs` today is a 25-field `Theme` const with five roles sharing a colour, and `DEFAULT_THEME` is referenced from ~15 widgets.

- Add `resolve(SemanticColor, Preset) -> ratatui::style::Color`.
- Presets `dark` / `light` / `terminal` (`Color::Reset` + ANSI-16). Truecolor detection from `COLORTERM`; without it, presets degrade to the 16-colour arm rather than emitting unsupported RGB.
- **`Theme` becomes derived from `resolve`, not a parallel table.** Keeping the 25 named fields is fine — what is not fine is two tables of colour facts (判据 §1). Every field's value comes from a `SemanticColor`.
- `/theme <name>` persists through the existing settings path.

**Guard — when does it go red:** a test iterating `ALL_SEMANTIC_COLORS` resolves each role in each preset and asserts no role falls through to a placeholder. Adding a 27th `SemanticColor` in `shared-ui-logic` turns this red until the TUI paints it. A second test asserts the legacy `Theme` fields are byte-equal to `resolve` for their role — the thing that makes "derived" true rather than claimed.

### B2 — Transcript model: chronological entries ✅

`ChatMessage` (`app/mod.rs:190`) is `User | Assistant{content,tools:Vec<ToolExecution>,reasoning,is_streaming} | System`. Tools hang *under* an assistant message, so they can only ever render above its text.

- Replace the message list with `Vec<TranscriptEntry>`; tool rows become peers of text, in arrival order.
- `ToolExecution` / `ToolStatus` are absorbed by `ToolRow` / `RowStatus`. `ToolStatus::Unknown` maps to `RowStatus::Pending` — the same "never a spinner that turns forever" rule `settle_resumed` already encodes, so the reconciliation `RunComplete` does against `summary.tool_summaries` keeps working.
- `finish_tool_execution` keeps the result text (≤ 64 KB per row); anything longer is fetched on expand through `trace.tool_output`.
- Touches `app/mod.rs`, `app/events.rs`, `app/trace.rs`, `commands.rs`, `widgets/chat_area.rs`, and `app/tests.rs` (2 935 lines).

**Guard:** a test that feeds `text → tool_start → text → tool_end` and asserts the rendered order is the arrival order — red today, because today's model cannot express it.

### B3 — Tool rows replace the bordered block ✅ (grouping deferred)

Delete `widgets/tool_block.rs` (221 lines, 3–5 lines of box per call).

```
⏺ Read(src/gateway/mod.rs:1-120)
  ⎿ Read 120 lines
⏺ Bash(cargo test -p aleph-tui)  ✗ 1.2s
  ⎿ error[E0433]: failed to resolve …
    … +41 lines (ctrl+o to expand)
⏺ Edit(src/tui/theme.rs)  +12 -3
  ⎿  30 │ - pub const DEFAULT_THEME
     30 │ + pub fn resolve(role: SemanticColor)
    … +2 hunks (ctrl+o to expand)
● Explored 4 files · 0.8s
```

- Header from `CallSummary`; status glyph and duration from `RowStatus`; body through `fold` with `FoldPolicy::for_display_name`, and through `diff_rows` when `RowBody::FileChanges`.
- `RowBody::FileChanges` with `unavailable` renders the reason, never a fabricated `Created` (spec §8).

**Guard:** a 6 KB single-line JSON body asserts the hint counts **physical** rows, not logical lines — the assertion `fold` already makes in `shared-ui-logic`, re-made at the surface that decides the width.

### B4 — Markdown: pulldown-cmark + `md_enhance` ✅ (syntect deferred to B4c)

Replace the hand-written 837-line `markdown.rs` subset (no tables, no highlighting).

- `md_enhance::enhance` first, then pulldown-cmark → `Line`/`Span`, with the **same flag constant the Panel uses**.
- Tables, ordered/nested lists, strikethrough, task lists, `hr`, block quotes, admonition left rail, dimmed link URLs.
- Mermaid fences render as a framed source block (R-4: no ASCII rendering in the TUI).
- **B4c, not done here:** `syntect` loaded lazily on a background thread; until it is ready, code blocks render as plain text and never block a frame. Highlighted results memoised per line.

**Guard:** the flag constant is imported from one place, and a test asserts the TUI's parser options equal the Panel's — a wire-shaped fact with two holders otherwise (判据 §10).

#### What the count turned up

The plan said "the same flag constant the Panel uses", which assumed two
holders. There are **six** `pulldown_cmark` parser construction points, and
three of them claim to hold the same fact:

| Site | Flags | Verdict |
|---|---|---|
| `webchat/components/markdown.rs` | `STRIKETHROUGH TABLES TASKLISTS` | same fact |
| `src/export/markdown.rs` | same, justified by the comment *"Same extension set as the Panel renderer"* | same fact |
| `cli/output/markdown.rs` | `STRIKETHROUGH TABLES` — **drifted** | same fact |
| `pdf_generate/{browser,native}_engine.rs` | `Options::all()` | different question |
| `webchat/memory_graph/markdown_excerpt.rs` | none | different question |

So the fact now lives in `shared_ui_logic::transcript::md_flags`. The Panel,
the CLI and the TUI call it; `alephcore` cannot (that crate has
`shared-ui-logic` only as a **dev**-dependency, and promoting it would point
core at an interface crate), so the exporter keeps a local copy pinned by a
test that reads the real constant instead of restating its three flags.

Two live defects fell out of the count:

1. The CLI never rendered task lists. `Event::TaskListMarker` has had an arm
   in that renderer since it was written, but `ENABLE_TASKLISTS` was not in
   its flag set, so the arm was unreachable and `- [x] done` printed as the
   literal text `• [x] done` — both ends built, no wire (判据 §7). The
   regression test uses an **uppercase** `[X]`, because a lowercase source
   renders byte-identically whether or not the marker was parsed; the first
   version of that test passed before the fix.
2. `src/export/markdown.rs` justified its flags by naming another
   subsystem's behaviour. That claim was already false for the CLI's copy.

#### The streaming seam had to change with the parser

`safe_freeze_offset` advances past any complete line outside a fence. That is
right for a line scanner — which this renderer was — and wrong for a block
parser: a frozen prefix cut mid-paragraph is two paragraphs forever, and a
half-arrived table is a header row plus literal pipes forever.

`chat_area`'s `build_visible_lines_window_matches_the_full_build_sliced`
caught it as a one-row disagreement between the streaming and settled builds,
which is a clipped last line in the scroll window, not a cosmetic difference.

Fixed by adding `block_freeze_offset` — the same scan, a second answer, cut
only after a blank line or a closing fence — and a seam that re-inserts the
blank row neither half can emit. The Panel still takes the line boundary; its
render is transient (a settled message re-renders whole), so it flickers
rather than freezing a mistake. Moving it over is a one-line change and a
measurement this session cannot make.

#### Deliberately different from the old renderer

Soft breaks now join into one paragraph, as CommonMark says. The old scanner
emitted one output row per source row, so an assistant paragraph that wrapped
in the source rendered pre-broken. Transcripts will look different; this is
the correct behaviour, not a regression.

### B5 — Mouse, region table, scrolling

- Enable crossterm mouse capture; wheel scroll; single click on a hint expands that row, double click (≤ 400 ms) collapses.
- Build a `RegionTable{rect, kind, row_id}` while painting, priority: show-more > back-to-bottom > fold hint > expanded card.
- Replace `auto_scroll: bool` with `shared_ui_logic::state::chat_scroll` (it is the stronger twin; the bool is its weakened copy — 判据 §1).
- Docked `[ ↓ Back to bottom · Ctrl+End ]` + a `Scrollbar`.
- Capture failure is not fatal: the keyboard path is unchanged and `expand_hint(Modality::Key)` supplies the wording.

**Guard:** a test asserts that with mouse capture off the hint text is the `ctrl+o` form — the fail-closed branch of spec §8, which is otherwise only reachable on a terminal nobody tests on.

### B6 — Chrome

- Header as the first transcript entry: `ℵ Aleph 26.x · <model> · ~/cwd (branch)`.
- Input: drop the full box; flat rule + gold `❯` + dim `Try "…"` when empty; grows with content.
- Status line `● model │ ctx 23% (50k/200k) │ $0.042 │ tier·mode·think │ ⚡2 │ ⠹ Pondering… 12s`, dropping segments by priority when the width runs out.
- Hint line `⏵⏵ auto · ctrl+o expand · ctrl+end bottom`, compressed to mode tags while typing.
- Cost = `session.usage` + accumulated `RunSummary.estimated_cost_usd`. Verb per turn from `affordance::verb`, re-rolled every 7 s; `✻ Worked for 12s` on completion.

**Guard:** a narrow-width test asserts the status line drops segments in the declared priority and never exceeds the width — the one thing a format string will get wrong silently.

### B7 — `/context` overlay, `/theme` command

- `/context` calls `context.breakdown` (Phase A RPC, zero clients so far), renders `reconcile` rows with proportion bars, and shows `?` for an unknown — never `0` (spec §8).
- `/theme <name>` switches and persists.

**Guard:** a `provider_reported: None` fixture must render `?`; asserting on `0` would pass on a broken path.

### B4c — Lazy syntax highlighting

Split out of B4 because it is a concurrency question, not a parsing one.

- `syntect` loaded on a background thread; until it is ready, code blocks
  render plain and never block a frame. Results memoised per line.

**Guard:** a test that renders a code block with the highlighter deliberately
not-yet-ready must produce the same rows as one with no highlighter at all —
"still loading" must not be a different layout, or the transcript reflows when
the load lands.

### B8 — Entropy reduction (same phase, separate commit)

- ~~Delete `widgets/tool_block.rs`~~ (done in B3) and ~~the old `markdown.rs` body~~ (done in B4).
- Delete `btw_panel.rs`'s second spinner (`affordance::spinner_frame` is the one source).
- Delete the three stale `#[allow(dead_code)]` in `AgentPanelData`.
- Done along the way, because the compiler named them: `Theme::code_bg`,
  `Theme::link`, `Theme::quote`, `Theme::code_block_border` had no readers left
  once the renderer moved to `palette().color(role)`. Note that
  `cargo test -p aleph-tui` hides these — the derivation test reads every field,
  so only a non-test build reports them (判据 §18).

## 3. Deliberately not done (spec §6)

Hover highlight, keyboard per-line cursor, side-by-side diff, ASCII mermaid, inline (non-fullscreen) mode.

## 4. Verification

```
cargo test -p aleph-tui -p aleph-cli
cargo clippy --workspace --all-targets      # after `just _stage-shell-placeholders`
```

Plus the six-command minimum set from CLAUDE.md when a change reaches outside `interfaces/tui/`, and a real-machine pass in Windows Terminal for mouse / truecolor / `/context` / `/theme`.

Host notes that apply (from memory): the `alephcore --lib` build outlives the 10-minute Bash ceiling — detach it; `--test '*'` needs `-j 1`; a green test binary can come from another worktree, so check what was actually rebuilt.
