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

## 2. Tasks

Ordered so the tree compiles after each and every commit is independently reviewable.

### B1 — Theme: one role table, three presets, `/theme`

`interfaces/tui/src/tui/theme.rs` today is a 25-field `Theme` const with five roles sharing a colour, and `DEFAULT_THEME` is referenced from ~15 widgets.

- Add `resolve(SemanticColor, Preset) -> ratatui::style::Color`.
- Presets `dark` / `light` / `terminal` (`Color::Reset` + ANSI-16). Truecolor detection from `COLORTERM`; without it, presets degrade to the 16-colour arm rather than emitting unsupported RGB.
- **`Theme` becomes derived from `resolve`, not a parallel table.** Keeping the 25 named fields is fine — what is not fine is two tables of colour facts (判据 §1). Every field's value comes from a `SemanticColor`.
- `/theme <name>` persists through the existing settings path.

**Guard — when does it go red:** a test iterating `ALL_SEMANTIC_COLORS` resolves each role in each preset and asserts no role falls through to a placeholder. Adding a 27th `SemanticColor` in `shared-ui-logic` turns this red until the TUI paints it. A second test asserts the legacy `Theme` fields are byte-equal to `resolve` for their role — the thing that makes "derived" true rather than claimed.

### B2 — Transcript model: chronological entries

`ChatMessage` (`app/mod.rs:190`) is `User | Assistant{content,tools:Vec<ToolExecution>,reasoning,is_streaming} | System`. Tools hang *under* an assistant message, so they can only ever render above its text.

- Replace the message list with `Vec<TranscriptEntry>`; tool rows become peers of text, in arrival order.
- `ToolExecution` / `ToolStatus` are absorbed by `ToolRow` / `RowStatus`. `ToolStatus::Unknown` maps to `RowStatus::Pending` — the same "never a spinner that turns forever" rule `settle_resumed` already encodes, so the reconciliation `RunComplete` does against `summary.tool_summaries` keeps working.
- `finish_tool_execution` keeps the result text (≤ 64 KB per row); anything longer is fetched on expand through `trace.tool_output`.
- Touches `app/mod.rs`, `app/events.rs`, `app/trace.rs`, `commands.rs`, `widgets/chat_area.rs`, and `app/tests.rs` (2 935 lines).

**Guard:** a test that feeds `text → tool_start → text → tool_end` and asserts the rendered order is the arrival order — red today, because today's model cannot express it.

### B3 — Tool rows replace the bordered block

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

### B4 — Markdown: pulldown-cmark + `md_enhance`

Replace the hand-written 837-line `markdown.rs` subset (no tables, no highlighting).

- `md_enhance::enhance` first, then pulldown-cmark → `Line`/`Span`, with the **same flag constant the Panel uses**.
- Tables (pi's column algorithm), ordered/nested lists, strikethrough, task lists, `hr`, block quotes, admonition left rail, dimmed link URLs.
- `syntect` loaded lazily on a background thread; until it is ready, code blocks render as plain text and never block a frame. Highlighted results memoised per line.
- Mermaid fences render as a framed source block (R-4: no ASCII rendering in the TUI).

**Guard:** the flag constant is imported from one place, and a test asserts the TUI's parser options equal the Panel's — a wire-shaped fact with two holders otherwise (判据 §10).

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

### B8 — Entropy reduction (same phase, separate commit)

- Delete `widgets/tool_block.rs` and the old `markdown.rs` body.
- Delete `btw_panel.rs`'s second spinner (`affordance::spinner_frame` is the one source).
- Delete the three stale `#[allow(dead_code)]` in `AgentPanelData`.

## 3. Deliberately not done (spec §6)

Hover highlight, keyboard per-line cursor, side-by-side diff, ASCII mermaid, inline (non-fullscreen) mode.

## 4. Verification

```
cargo test -p aleph-tui -p aleph-cli
cargo clippy --workspace --all-targets      # after `just _stage-shell-placeholders`
```

Plus the six-command minimum set from CLAUDE.md when a change reaches outside `interfaces/tui/`, and a real-machine pass in Windows Terminal for mouse / truecolor / `/context` / `/theme`.

Host notes that apply (from memory): the `alephcore --lib` build outlives the 10-minute Bash ceiling — detach it; `--test '*'` needs `-j 1`; a green test binary can come from another worktree, so check what was actually rebuilt.
