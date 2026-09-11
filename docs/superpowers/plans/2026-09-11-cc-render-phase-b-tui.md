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

**Phase B is complete: B1–B8 landed.** B1/B2/B3 — `6594bfca6` (shared `at_ms`) +
`071fac0f0` (the TUI). B4 — `3db635bed` (one flag set across four renderers) +
`9e0c8eb7f` (lazy syntect). B5 — mouse, region table, `chat_scroll`
(`4b5648464`, `1b91f285e`). B6 — header, boxless composer, hint line,
width-aware status line, cost (`d94e67e95`) plus an entropy commit
(`9712a87c3`). B7 — the `/context` overlay and a remembered `/theme`
(`898d0ec16`, `c546d476f`, `5f1b4f547`). B8 — the spinner tables and the stale
suppressions. 434 `aleph-tui` tests green, zero warnings in both test and
non-test builds; `aleph-cli` (241) and `shared-ui-logic` (154) re-verified. B6
touched only `interfaces/tui/`; B7 also touched `shared/ui_logic` (one label),
so `shared-ui-logic` was re-run for it and `aleph-panel --lib --no-run` was
built to prove the label change reaches nothing over there (it does not call
`reconcile` at all — Phase C has not started). B8 touched `interfaces/tui/` and
`interfaces/cli/` and changed nothing in `shared/ui_logic`, so the Panel build
was not repeated.

**Each of B5 and B6 found a severed wire it did not create.** B5: `ctrl+o`
had been printed under every folded tool row since B2, bound to nothing, and
`ToolRow::expanded` had no writer in the crate. B6: `TranscriptEntry::TurnSummary`
had a renderer in `chat_area` since B3 and no producer anywhere, and
`last_run_duration` was written at every run end and rendered nowhere. Both
sections below have the detail. The pattern is now three for three: **a round
that adds a renderer, a label or a field should grep for the other end in the
same sitting** — dead-code analysis is structurally blind to this shape,
because both ends exist. B7 ran that grep and found no new severed wire; what
it did find was a *weakened copy* — B6's own hand-rolled `home_dir()`, the
shared one minus an arm (判据 §1). The rule generalises: the sweep is for the
fact, not only for the wire.

**B8 then found that a plan bullet is itself a count, and counts here are short
by one.** The bullet named *one* second spinner (`btw_panel.rs`). Grepping for
the glyphs instead of for the file name found `interfaces/cli/src/output/
spinner.rs` holding a third copy of the braille table *and* a second copy of the
80 ms period — beside a fourth copy of the same ASCII frames `btw_panel.rs` was
being deleted for (判据 §6: 数错的方向永远是少一个). Neither client compares
its spinner to the other's, so the drift would have been silent in the only
place it shows: two Aleph surfaces open side by side.

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
4. ~~**`/theme` persists through `ALEPH_TUI_THEME`, not a settings file.** R4:
   an interface does not persist, and this crate has no config of its own. A
   terminal's colour preference is the same kind of fact as `COLORTERM`.~~
   **Overturned in B7** — see that section. The R4 reading was too wide, and
   the Panel, an interface under the same redline, persists its six appearance
   axes to `localStorage`. `/theme` now writes `<aleph_home>/tui-theme`;
   `ALEPH_TUI_THEME` still outranks it for a single run.

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

### B5 — Mouse, region table, scrolling ✅

- Enable crossterm mouse capture; wheel scroll; single click on a hint expands that row, double click (≤ 400 ms) collapses.
- Build a `RegionTable{rect, kind, row_id}` while painting, priority: show-more > back-to-bottom > fold hint > expanded card.
- Replace `auto_scroll: bool` with `shared_ui_logic::state::chat_scroll` (it is the stronger twin; the bool is its weakened copy — 判据 §1).
- Docked `[ ↓ Back to bottom · Ctrl+End ]` + a `Scrollbar`.
- Capture failure is not fatal: the keyboard path is unchanged and `expand_hint(Modality::Key)` supplies the wording.

**Guard:** a test asserts that with mouse capture off the hint text is the `ctrl+o` form — the fail-closed branch of spec §8, which is otherwise only reachable on a terminal nobody tests on.
Landed as `chat_area::click_tests::with_mouse_capture_off_the_hint_names_the_key`, which drives a real
`render_chat_area` and reads the painted row; mutation-verified by making `AppState::modality` return
`Mouse` unconditionally.

#### Ctrl+O was advertised on every row and bound to nothing

`KEY_MODALITY` has read `Modality::Key("ctrl+o")` since B2, so every folded tool row in the transcript
has been telling the reader to press a key `keys.rs` had no arm for, and `ToolRow::expanded` had no
writer anywhere in the crate. Both ends built, no wire (判据 §7) — and the searchable evidence was the
hint string itself, which reads exactly like a working feature. B5 supplies the arm.

It is a **bulk edit of the per-row `expanded` flags**, not a second sticky "expand everything" flag:
two holders of "is this row unfolded" would be two answers to one question (判据 §1), and the row's own
flag is the one the renderer and the click path already read. The visible consequence of choosing this
way is that a tool row arriving *after* the keystroke arrives folded — wearing a hint that is true of it.

#### Five deviations from this section as written

1. **No 400 ms double-click timer.** Expanding deletes the hint the gesture was made on, so a second
   click 400 ms later lands on whatever row moved into that cell — the collapse branch could never have
   fired. Replaced by a plain toggle with two targets per row (see 2), which satisfies "click expands,
   click again collapses" without a timer, a clock, or a flaky test.
2. **Three click targets per row, not one.** The header toggles too, and an *unfolded* row closes with
   `… (ctrl+o to collapse)` — a new `affordance::collapse_hint`, beside the expand wording so both
   surfaces keep reaching it from one place. This was measured, not reasoned: the first version had the
   header as the only surviving target, and
   `clicking_the_hint_unfolds_and_the_closing_hint_folds_again` failed with the header scrolled off the
   top, because a body taller than the viewport pushes it there the instant it unfolds.
3. **Two `RegionKind`s, not four.** `show-more` and `expanded card` name regions this TUI does not
   paint. Enumerating them would be two arms nothing can produce and nothing can be observed to handle
   (判据 §17). The priority order is a `match` on the kind rather than declaration order, so a third
   kind has to answer the question rather than inherit an answer from where it was typed.
4. **`auto_scroll` was deleted outright, not ported.** `visible_window`'s `auto_scroll == true` branch
   computed exactly `(total − height, total)`, which is what the offset branch already yields at
   `scroll_offset == 0`. The bool was a weakened second spelling of "parked at the bottom" from birth.
   `stuck_to_bottom()` is now `scroll_offset == 0`, and `chat_scroll::scroll_action` decides what may
   override it — which also buys the case the old shape could not express: **the viewer sending while
   scrolled up**, whose answer used to stream in off-screen. `settle_scroll` runs once per main-loop
   iteration and the eighteen `ScrollToBottomIfAutoScroll` sites (and the `Action` variant) are gone.
   `MarkUnseen` reaches the screen as the docked control's second wording, so no arm is decorative.
5. **`Wrap` is off and every painted row is clipped to the pane** (`chat_area::clip_to_width`). This
   widget sizes its scroll window in *logical* lines and now records click targets in them, so a line
   that painted as two rows would slide every target below it — a latent defect the region table would
   have inherited. Three producers never wrapped on their own: a tool header (one unbreakable
   `Bash(…)`), a diff row (its gutter would have to be re-emitted on a continuation), and a turn
   summary (a sentence that simply gets long). Enforced once, at the paint site that knows both the
   line and the pane, rather than asked of each of them (判据 §12).

#### Two guards of my own that were green for the wrong reason

- A first `no_painted_row_is_wider_than_the_pane` called `clip_to_width` itself and so proved the
  function agrees with a copy of itself (判据 §10). Deleted and replaced with
  `an_over_wide_row_above_a_tool_row_does_not_shift_its_click_target`, which reads the painted buffer.
  Its first fixture used a *system message* — which wraps itself — and stayed green under both
  mutations; it is a tool header now.
- A pre-pass scroll clamp against the cache's last-known heights survived its own removal (the test
  stayed green), because the post-build clamp already covered every case. Deleted: a second derivation
  that can never be the one that fires (判据 §2, §12). The surviving clamp is mutation-verified.

**Cost to note:** with capture on, the terminal stops handling drag-select itself, so copying text out
of the transcript needs the emulator's override (Shift+drag in most). Whether a given Windows Terminal
profile honours that is on the real-machine list, unmeasured.

### B6 — Chrome ✅

- Header as the first transcript entry: `ℵ Aleph 26.x · <model> · ~/cwd (branch)`.
- Input: drop the full box; flat rule + gold `❯` + dim `Try "…"` when empty; grows with content.
- Status line `● model │ ctx 23% (50k/200k) │ $0.042 │ tier·mode·think │ ⚡2 │ ⠹ Pondering… 12s`, dropping segments by priority when the width runs out.
- Hint line `⏵⏵ auto · ctrl+o expand · ctrl+end bottom`, compressed to mode tags while typing.
- Cost = `session.usage` + accumulated `RunSummary.estimated_cost_usd`. Verb per turn from `affordance::verb`, re-rolled every 7 s; `✻ Worked for 12s` on completion.

**Guard:** a narrow-width test asserts the status line drops segments in the declared priority and never exceeds the width — the one thing a format string will get wrong silently.
Landed as `status_bar::tests::a_narrow_line_sheds_segments_in_the_declared_order` (the pure
function, at every width from 8 to 120) plus
`what_survives_the_fit_is_what_reaches_the_screen` (the painted cells). Mutation-verified by
disabling the drop loop and by inverting `max`→`min` on the priority.

#### Six deviations from this section as written

1. **The header's folder is `SessionSnapshot.project_root`, and it is absent when the server
   reports none.** `~/cwd` cannot mean the terminal's own working directory here: the gateway
   may be on another machine, and when it is not, the agent's cwd is still its own
   `~/.aleph/workspaces/{agent_id}` rather than wherever the TUI was launched. The client is
   told exactly one directory — the conversation's `project_root` — so that is the one shown,
   and a conversation scoped to nothing shows nothing (判据 §1's fourth face: a line that
   describes another subsystem's state).
2. **The branch is read from this machine's filesystem, and that is the one piece that can be
   wrong.** No RPC reports it. `git_branch_of` reads `<project_root>/.git/HEAD`, following the
   `gitdir:` pointer a worktree leaves (this repository is one), and yields `None` for anything
   it cannot read — which is what a remote gateway's path produces. The residual case is
   documented at the function: a remote gateway whose `project_root` *also* exists here.
3. **`aleph-tui` gained a `build.rs`.** The header names a version and this crate had no
   `ALEPH_VERSION`: it must not depend on `alephcore`, and `CARGO_PKG_VERSION` is the
   workspace number kept in sync with the `VERSION` file *by hand*. Same script, same reason,
   as `interfaces/cli/build.rs` — a third reader of the one source of truth, not a third
   source.
4. **The status line kept its existing segments.** The spec's example line does not show
   `cache N%`, the session key, the token count or `T:`, but each of those is a live signal
   someone added for a reason (a cache drop is how a prefix bust announces itself). They are
   segments with priorities rather than deletions; the width logic is what the section asked
   for, and it is what decides which of them a narrow terminal keeps.
5. **The hint line names keys only; `⏵⏵ auto` is not on it.** The exec tier already has exactly
   one home, on the status line. Rendering it twice is the same fact in two places waiting to
   disagree (判据 §1). The "compressed while typing" form is the send/newline keys instead —
   which is a better answer to the same need, because those two key names had been living in
   the input box's title and the box is what this task deletes.
6. **`✻ Worked for 12s` and the turn summary are two rows, not one.** They are two clocks: the
   summary sums the tools' own durations, the trailer is the run's wall clock, and the gap
   between them is the time the model spent thinking. Collapsing them would have to pick one
   and be wrong about the other.

#### The welcome message is gone, and that is the point

`AppState::new` opened every session with `Welcome to Aleph CLI. Session: … | Model: … Type
/help for commands.` All three of its facts now have one live home each — the model on the
derived header, the key on the status bar, `/help` on the hint line — and the entry was a
frozen string: the model it named was correct only until the first session snapshot arrived,
about a second later. The transcript now starts empty and the header is derived per frame
rather than stored, so `/session` and a model switch move it.

#### Two things B6 found rather than built

- **`TranscriptEntry::TurnSummary` had a renderer and no producer.** `chat_area` has rendered
  it since B3; nothing in the crate ever constructed one outside a test. Same shape as B5's
  `ctrl+o` (判据 §7). A completed run now emits one via the shared `summarize_turn`, counted
  over the rows `RunSummary.tool_summaries` names so it describes *that run* rather than
  everything still on screen.
- **`last_run_duration` was written at every run end and read by nothing** but the test
  asserting it had been written (判据 §17). Deleted in the entropy commit; the duration
  reaches a reader now as the trailer.

#### One guard of my own that was green for the wrong reason

`what_survives_the_fit_is_what_reaches_the_screen` first asserted that at 40 columns the
session key is *absent*. It stayed green with `fit` disabled entirely — a 40-column buffer
cannot hold the session key whether it was dropped or clipped, so the assertion was measuring
the backend's width (判据 §10). What only dropping can produce is the segment to the *right*
of the shed ones becoming visible, so it now asserts the working indicator is on screen at 40
columns. That version reddens under the mutation.

**Cost to note:** the composer is two rows shorter than the boxed version at its minimum, and
the hint line takes one of them back — net one row returned to the transcript. The chat area
still has its own box; only the input lost one.

### B7 — `/context` overlay, `/theme` command ✅

- `/context` calls `context.breakdown` (Phase A RPC, zero clients so far), renders `reconcile` rows with proportion bars, and shows `?` for an unknown — never `0` (spec §8).
- `/theme <name>` switches and persists.

**Guard:** a `provider_reported: None` fixture must render `?`; asserting on `0` would pass on a broken path.

Landed as `app/context_view.rs` (the join), `widgets/context_overlay.rs` (the
paint), a `Focus::Context` key path, and `theme.rs`'s remembered preset.
`aleph-tui` 433, `aleph-cli` 241, `shared-ui-logic` 154, clippy `-D warnings`
clean on both touched crates.

#### The join the three doc comments asked for

`context.breakdown` **always** sends `provider_reported: None` — it measures
the prompt, and only a provider's response carries a provider's count — and
`reconcile` turns that `None` into `total: None` / `percent: None` rather than
dressing our own sum as the provider's. So a client that renders the response
verbatim gets rows with no total and no bar. The wire type, the handler and
`reconcile` each say so; **B7 is the first client that exists**, and
`ContextView::new` is that join.

Two choices inside it that are not obvious from the diff:

- **The whole gauge occupancy goes into `UsageTokens::input`.** The struct has
  four fields because that is the shape a provider reports; the gauge is one
  number. `reconcile` sums `input + cache_read + cache_creation`, so this is
  what makes the sum equal the gauge. Splitting it across the cache fields
  would invent a cache breakdown this client does not have.
- **The gauge's window overwrites the server's.** They are meant to be the
  same number, and if they ever differ the screen must not show `/context`
  disagreeing with the status bar one row below it about the size of this
  window (判据 §12). The server's answer stays as the fallback for a session
  that has not run yet.

The rows therefore describe the **system prompt**, and the remainder — the
conversation, the last response, anything else in the window — lands in
`reconcile`'s `Other`. `Other` is the honest name for it: this view never
measured those bytes, and labelling the row "Messages" would be a specific
claim about a number nobody counted (判据 §17). The footer says so in words.

#### Four deviations

1. **`/theme` persists, and §1b's item 4 is overturned.** That entry said R4
   forbade it. The twin says otherwise (判据 §16): the Panel is an interface
   under the same redline and persists its six appearance axes to
   `localStorage`. R4 forbids an interface holding *business* state — sessions,
   memory, plans — and a per-device display preference is what `localStorage`
   is for. `<aleph_home>/tui-theme` is this surface's `localStorage`; the path
   hangs off `aleph_protocol::paths::aleph_home`, so `ALEPH_HOME` isolates a QA
   instance from the developer's own preference. `ALEPH_TUI_THEME` still wins,
   because that is the form a script or a one-off invocation can use. **The
   file is not read in this crate's test binary**: a maintainer who once ran
   `/theme terminal` would otherwise run every colour-sensitive test in a
   different palette from CI (判据 §18), and the precedence rule is tested with
   both sources as parameters instead.
2. **The overlay is sized to its content, not to a fraction of the frame.** A
   fixed 80% box leaves a tall empty rectangle under a three-layer prompt and
   clips a thirty-layer one just the same.
3. **The footer is two short lines rather than one long one.** The overlay does
   not wrap — a row that wraps stops being a table — so a long footer is a
   *clipped* footer, and a clipped label is a wrong label, not a shorter one.
   Found by looking at the paint, not by a test.
4. **A row under one percent reads `<1%`.** A bare `0%` beside a visible sliver
   of bar says the row costs nothing, which is a different claim.

#### One guard of mine that was green for the wrong reason, again

`a_short_overlay_keeps_the_headline_and_the_footer` first used the same
two-layer fixture as its neighbours. With four rows the list is short enough
that a *wrong* row budget still leaves the footer on screen, so the test passed
without measuring anything (判据 §10) — verified: `inner.height - 4` → `- 2`
left it green. It now uses a twelve-layer fixture, which reddens under that
mutation. Eight mutations in total, each producing the expected red name.

#### Two things found along the way

- **B6 wrote its own `home_dir()`** in `widgets/header.rs`: `HOME` then
  `USERPROFILE`, which is `aleph_protocol::paths::home_dir` **minus its
  `HOMEDRIVE` + `HOMEPATH` arm** — a copy born weaker than its original, and
  one that agrees with it on every machine except the ones the missing arm
  exists for (判据 §1). Now delegates.
- **`local_commands_returns_catalog` asserted `cmds.len() == 22`.** A count
  that has to be hand-bumped can only ever catch the author forgetting to bump
  it; it cannot catch an advertised word the parser does not know. Replaced by
  exactly that check, over every entry.

Also fixed in passing: `reconcile`'s `Tools (1 schemas)`.

### B4c — Lazy syntax highlighting ✅

Split out of B4 because it is a concurrency question, not a parsing one.

- `syntect` loaded on a background thread; until it is ready, code blocks
  render plain and never block a frame.

**Guard:** a code block rendered with the highlighter not-yet-ready produces
the same rows as one rendered with it — "still loading" must not be a
different layout, or the transcript reflows when the load lands.
Mutation-verified: making the highlighted path skip `wrap_line_spans` (the
natural shortcut, since it already has its spans) turns it red at width 30.

Four things worth recording:

1. **No per-line memo cache.** The plan asked for one; `chat_area`'s
   `LineCache` already caches a settled message's rendered rows, so a second
   cache would be a second copy of the same idea (判据 §1). Invalidation
   instead rides on a `highlight_generation` key in the cache entry — the load
   landing is not visible in a message's own inputs, so without it a
   transcript keeps whichever state each message happened to be rendered in
   and never converges.
2. **The `terminal` preset is never highlighted**, and neither is a terminal
   without truecolor. `Palette::is_rgb()` already answers both, so the gate is
   that one call rather than a second enumeration of presets.
3. **The palette is a parameter**, not an ambient read, so the per-preset tests
   need no global mutation — which in this test binary would race every other
   test that reads a colour.
4. **Two guards were green for the wrong reason first.** The ambient palette
   in a test binary paints no RGB, so the row-equivalence test compared plain
   against plain until it was given an explicit RGB palette and a synchronous
   load; and the two highlighter tests re-ran `HighlightLines` themselves
   instead of calling the function under test, which only proves the logic
   agrees with a copy of itself.

**Cost to note:** `aleph-cli` depends on `aleph-tui`, so the CLI binary now
carries syntect's syntax definitions too.

### B8 — Entropy reduction (same phase, separate commit) ✅

- ~~Delete `widgets/tool_block.rs`~~ (done in B3) and ~~the old `markdown.rs` body~~ (done in B4).
- ~~Delete `btw_panel.rs`'s second spinner~~ — and, because the sweep was for
  the glyphs rather than for the file, **two more copies in
  `interfaces/cli/src/output/spinner.rs`**: a third spelling of the braille
  table and a second spelling of the 80 ms period. All three now read
  `SPINNER_FRAMES` / `SPINNER_PERIOD_MS` from `shared-ui-logic`. Output is
  byte-identical in every case; that is the point of the change, not a caveat
  to it.
  - `btw_panel.rs`'s table was ASCII while every other spinner on the same
    screen is braille, under a doc comment reading "matching the status bar's
    cadence" — it matched neither the sequence nor the period. 判据 §1's fourth
    shape, where the copy that lies is the comment.
  - The CLI's `ASCII_FRAMES` **stays local**: `use_unicode()` is that crate's
    gate and no other surface offers the fallback, so it has exactly one
    holder. Moving it to the shared crate would create a second home for a
    fact with one reader.
  - Guard: `the_status_glyph_comes_from_the_shared_table` asserts against
    `spinner_at(tick)` rather than a literal frame, over ticks that run past
    the table's length so a table of a different *size* fails too. Mutation
    (restore the four ASCII frames) → that one name red, nothing else.
  - The CLI half gets **no** test. After the change the two expressions are the
    same expression; an assertion would be reading its own value and could not
    go red (判据 §2, 恒绿). The compiler is the guard.
- ~~Delete the three stale `#[allow(dead_code)]` in `AgentPanelData`~~. Their
  stated reason — "read by Task 8b's widget, not by anything in Task 8a's
  scope" — stopped being true when 8b shipped. A suppression is a guard
  switched off: while it is there nothing about it can go red, so the only way
  to read one is to delete it and rebuild. Clean build ⇒ it was stale.
- Done early, in `9712a87c3`, because B6 walked into them: the **second
  `SessionKnob` enum** (`app/mod.rs`'s, a four-arm identity relabelling of the
  parser's, defended by a comment that described a wiring the code does not
  have) and **`last_run_duration`** (written every run end, rendered nowhere).
  Removing the enum also returned `AppState`'s doc comment to `AppState` —
  the enum had been inserted under the banner and took the `///` lines with
  it, leaving the struct undocumented.
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

Two instrument notes earned in this phase:

- **Run `cargo build -p aleph-tui`, not only `cargo test`.** The theme
  derivation test reads every `Theme` field, so a field with no production
  reader is invisible to a test build and reported only by a plain build.
  Three dead fields survived a green `cargo test` this way.
- **Pass rustfmt the crate's real edition.** These crates are edition 2021;
  `rustfmt --edition 2024` reorders import lists into a different sort and
  produces a diff that has nothing to do with the change.
- **A widget test cannot see width overflow in the buffer.** The backend is
  exactly `width` columns wide whether the paragraph wrapped, truncated or
  fitted, so "no row is too wide" has to be asserted on a *consequence* — a
  click target that did not move — not on the painted cells (判据 §2: a
  check that cannot go red).
- **Read the red NAMES before the count.** Two of B5's own guards were green
  for the wrong reason and only the mutation runs said so: one applied the
  function it was testing, one had a fixture that wrapped itself. A third
  guard survived its own deletion, which is how the redundant scroll clamp
  was found (判据 §18).
- **A narrow `TestBackend` cannot tell "dropped" from "clipped".** B6's
  status-line paint test first asserted that the session key is absent at 40
  columns — which stays green with the drop logic deleted, because a
  40-column buffer could not have held it either way. The assertion has to
  name something only dropping can produce: the segment to the RIGHT of the
  shed ones appearing. Same family as the width note above — the backend's
  own size is not evidence about the code that filled it.
- **A fixture too small for the budget under test proves nothing.** B7's
  "short overlay keeps its footer" test used a four-row list; at that size the
  footer stays on screen even with the row budget computed wrongly. The
  fixture has to be larger than what fits, or the test is measuring the
  fixture (判据 §10).
- **`python - <<EOF` silently does nothing on this host.** A mutation applied
  that way reported "19 passed" for *unmutated* code — the instrument said the
  guard was weak when it had never run (判据 §18). Verified after the fact with
  `diff`. Apply mutations with the editor, or with a script written to a file
  and run by path.
- **A suppression cannot be read while it is on.** `#[allow(dead_code)]` is a
  guard switched off: nothing about it can go red, so no build and no test run
  will ever tell you it went stale. The only instrument is deleting it and
  rebuilding. All three of `AgentPanelData`'s were stale, and had been since
  the task their `reason` strings pointed at shipped.
- **Workspace `cargo fmt --check` over-reports by about ninety files here.**
  It applies the root crate's edition to crates that are edition 2021 — the
  same trap as the note above, seen from the workspace end. The useful reading
  is *absence*: a file missing from that noisy list is formatted under any
  narrower rule. Confirm with `rustfmt --edition 2021 --check <paths>`.

## 5. Open after Phase B

Phase B is code-complete. What it deliberately leaves for whoever picks up next:

- **Nothing here has been seen on a real terminal.** Every paint in B5–B8 was
  verified against `TestBackend`, which is a grid of cells and not a terminal:
  it has no opinion about mouse capture, truecolor, or how wide `⏺`, `ℵ`, `❯`,
  `⏵⏵`, `✻`, `⚡`, `█` and `░` actually are. A Windows Terminal pass over
  mouse / truecolor / `/context` / `/theme` is UNRUN, not passed. Whether
  Shift+drag text selection still works with capture on is likewise unmeasured.
- **`trace.tool_output` is now the only zero-client Phase A RPC.** `transcript/`
  and `context.breakdown` both got clients in this phase.
- **Grouping is still deferred.** `group_entries` takes `Vec<TranscriptEntry>`
  by value, so calling it per frame reinstates the O(transcript) deep copy that
  `build_visible_lines` exists to avoid. Changing the signature is the
  prerequisite, not the caller.
- **Two duplications found by B8's sweep and left alone**, because both sit in
  files no task in this phase touched and fixing them would widen the diff into
  unrelated commands. Recorded here so the next sweep does not have to
  rediscover them:
  - `interfaces/cli/src/commands/heartbeat_cmd.rs` parses a `TaskState` whose
    five fields are never read, behind an `#[allow(dead_code)]` placed on the
    whole enclosing `TaskView` — wider than its reason, since five of
    `TaskView`'s own fields *are* read. Every field is `#[serde(default)]`, so
    the type cannot reject a malformed reply either: it is 判据 §2's
    "不可失败" face. Note the product question underneath it — `heartbeat list`
    shows no health column, and `consecutive_errors` / `last_error` are already
    on the wire.
  - Four spellings of "how long did it take" render the same transcript:
    `shared_ui_logic::transcript::fmt_duration_ms`, the CLI's
    `output::exec_echo::format_duration`, and two in the Panel
    (`views/cron/helpers.rs`, `views/subagent_tree/visuals.rs`). Same shape as
    `sanitize_link_event`, which `src/export/markdown.rs` and the Panel each
    hold a copy of — that one is security-relevant and was flagged to the user
    in B7.
