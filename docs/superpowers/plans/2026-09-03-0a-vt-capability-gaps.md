# 0-A · VT capability gaps: what extending `src/gateway/pty/screen/` would have to cover

**Status**: inventory, not a port plan. Read-only survey, 2026-09-03.
**Aleph side**: `src/gateway/pty/screen/{mod,perform,grid,convert,diff,text}.rs` (2 735 lines, of which
~440 are the `vte::Perform` impl at `perform.rs:1-415`; the rest is tests and the grid).
**herdr side**: `/Volumes/TBU4/Github/herdr` @ `main`.
**Constraint that frames every row**: R3/禁用清单 bans a second VT implementation. Every gap below is
closed by *extending* `screen/`, never by importing. Cost column is priced as an extension.

> **Status update, 2026-09-04 (terminal round 2, branch `worktree-terminal-round2`).** Every row below
> keeps its original wording — it is the record of what was true on 2026-09-03 — and carries a
> `SHIPPED` line where round 2 closed it. **Closed:** A2, A3, A4, A5, A6, A8, B5, C1, C5.
> **Partially closed, and the heading says which half:** B1 (OSC 7 only — no OSC 9;9, no OSC 1337)
> and C9 (`?25` only — no DECSCUSR shape, no `?12` blink). **Not closed: A7 (SCS / DEC Special
> Graphics).** The round-2 spec's §4.7 asked for "A2–A8" to be stamped, but its own §4.2
> implementation table has no A7 row and the code has no branch: `perform/esc.rs::esc` returns early
> on a non-empty intermediate, so `ESC ( 0` is dropped by construction. A status line claiming a
> capability the code does not have is worse than no status line (判据 §17), so A7 stays open —
> and this row's own advice ("wait for a capture that shows it firing") still stands.
> Round-2 落点索引 → [FEATURE_LOCATOR §6.11](../../reference/FEATURE_LOCATOR.md) ·
> 子系统全文 → [TERMINAL_RUNTIME.md](../../reference/TERMINAL_RUNTIME.md).

---

## Summary: the two emulators do not differ in *coverage*, they differ in *kind*

The first thing the survey turned up changes what "compare the two emulators" means: **herdr has no
Rust VT to compare against.** Its terminal is [libghostty-vt](https://github.com/ghostty-org/ghostty)
— vendored Zig at `vendor/libghostty-vt/` (`VERSION` = `1.3.2-HEAD-+c5a21edfc`), compiled by
`build.rs:32-60` with a `zig build` invocation and bound through 352 KB of bindgen output at
`src/ghostty/bindings.rs`. `src/pane/terminal.rs` is a 6 735-line *wrapper*: it owns a
`GhosttyPaneCore`, feeds it bytes at `src/pane/terminal.rs:1216` (`process_pty_bytes`), and reads
answers back out. And `src/terminal/state.rs` — the other file the redline names — is not VT code at
all: it is agent-state arbitration (`HookAuthority`, hook-vs-screen precedence,
`src/terminal/state.rs:18-45`). So the ban on "移植 herdr 的 `pane/terminal` + `terminal/state`" is, read
literally, a ban on adopting libghostty as Aleph's VT plus a ban on copying a state machine that
Aleph has **already ported independently** (`crates/agent-detect/`). Neither is a Rust VT.

That reframes the comparison usefully. Aleph's `screen/` is a **~440-line, purpose-built, output-only
text sampler** built on the `vte` crate: it models exactly the state that survives into
`visible_text()` + `title()`, and everything else falls through two catch-alls (`perform.rs:404` for
CSI, `perform.rs:351` for ESC) and one silent-by-omission surface (`hook`/`put`/`unhook`, which
`vte::Perform` defaults to no-ops). libghostty is a **full-fidelity, bidirectional, human-facing
emulator**: it answers device queries, tracks ~30 DEC modes (`vendor/…/src/terminal/modes.zig:253-299`),
stores kitty images, encodes keys back to the PTY, and keeps grapheme clusters intact. herdr *needs*
all of that because it paints pixels for a human and forwards a human's keystrokes. Aleph does not:
it samples text and a title and classifies a state.

The practical consequence is that most of the surface area difference is **irrelevant to Aleph, and a
small minority is not**. The gaps that matter are the ones that either (a) corrupt the *text*
`visible_text()` returns, or (b) starve a detection input that Aleph's already-ported manifest engine
is *already asking for*. Exactly one gap is in category (b), and it is the highest-value row in the
table: `osc_progress` has a consumer, three manifests with rules keyed on it, and a hardcoded `""`
where its producer should be.

**One methodological caveat, stated up front.** Because herdr's VT is Zig, "what herdr does" cites
two different kinds of anchor: herdr *Rust* lines where herdr itself implements or consumes
something, and `vendor/libghostty-vt/src/…` Zig lines where the behaviour lives in the vendored
library. I mark which is which on every row. Where I could not establish something I say so rather
than filling the cell.

---
## Tier A — Matters: corrupts `visible_text()`, or starves a consumer that already exists

Ordered by expected cost to Aleph (likelihood × severity), not by what herdr considers important.

### A1 · OSC 9;4 progress — the only gap with a consumer already waiting ✅ WIRED 2026-09-03

| | |
|---|---|
| **What herdr does** | Two independent paths. libghostty parses `OSC 9;4;<state>;<pct>` into a typed `conemu_progress_report` (`vendor/libghostty-vt/src/terminal/osc.zig:123-124`, payload struct at `:205-233`). Separately — and this is the path herdr actually uses — herdr runs its **own** always-on byte scanner over the raw PTY stream: `AgentOscStateTracker` at `src/pane/osc.rs:458-464`, whose `observe` retains `b"9"` payloads at `src/pane/osc.rs:487-490`. It surfaces as `agent_osc_progress()` (`src/pane/terminal.rs:1201`, re-exported `src/pane.rs:2780`) and is fed straight into detection at `src/pane.rs:930` and `src/pane.rs:2497`. |
| **What Aleph does today** | `osc_dispatch` **has no branch for it**: the guard at `perform.rs:411` is `matches!(*kind, b"0" \| b"2")`, and the `if` has no `else` — OSC 9 falls out the bottom of the function at `perform.rs:415`. `vte` *does* hand `["9", "4;3;"]` to `osc_dispatch`, so unlike herdr, Aleph needs no second scanner. Downstream, `gateway/runtime/mod.rs:40` defines `pub const OSC_PROGRESS_UNAVAILABLE: &str = ""` and passes it at `:160`; a test at `:348` pins that it stays empty. |
| **What breaks in practice** | **A severed wire, not a missing feature.** Aleph has already ported the manifest engine that consumes this. `crates/agent-detect/src/manifest.rs:980` routes `region = "osc_progress"`, and three shipped manifests key rules on it: `manifests/grok.toml:89-93` (`osc_progress_working`, **priority 1150 — the highest-priority rule in that file**), `grok.toml:119-124` (`osc_progress_idle`, priority 950), `qwen.toml:99-104` (`osc_tool_progress_working`, priority 850), `claude.toml:222-226` (`osc_progress_idle`, priority 250). Every one of them is unreachable today, so Grok falls all the way through to a priority-200 spinner regex and Qwen to a bottom-lines timer regex. Concretely: **a model reading the screen cannot tell a long build from a hung one** in exactly the case the manifest was written for — the agent painted no spinner this frame (mid-repaint, or output redirected) but did emit `\e]9;4;3;\a`, and Aleph reads that frame as "no evidence" instead of "working". |
| **Cost to extend** | **Small.** One `match` arm in `osc_dispatch` next to the existing `b"0" \| b"2"` arm, one `Option<String>` on `ScreenState`, one accessor beside `Screen::title()`, and replacing the constant at `runtime/mod.rs:160`. herdr needed 70 lines of hand-rolled OSC scanning because libghostty does not surface OSC 9 to embedders; `vte` does, so Aleph gets it for a fraction of the cost. Note the payload convention the manifests expect: the part **after** `9;`, i.e. `"4;3;"` not `"3"` — see `src/pane/osc.rs:487-490` and `grok.toml:93`'s `^4;1;-1$`. Sanitise like `sanitize_agent_osc_string` (`src/pane/osc.rs:536-545`): strip control chars, cap length (herdr uses 256, `src/pane/osc.rs:448`). |
| **SHIPPED** | Wired on 2026-09-03. `ScreenState.osc_progress` + `Performer::retain_osc_progress` + `Screen::osc_progress()` in `src/gateway/pty/screen/perform.rs`; `RuntimeAgents::sample` now reads it and `OSC_PROGRESS_UNAVAILABLE` is deleted. Five guards, each falsified by hand: the runtime wire (`the_osc_progress_wire_is_actually_connected` — cut the read and grok's `4;1;-1`/`4;0;0` collapse to one state), the namespace filter, the char cap, the control-char strip, and the payload shape. **Two claims in the rows above were wrong and are corrected here** (判据 §18 — my own instrument): (1) `vte` 0.14.1 does NOT hand over `["9", "4;3;"]` — it splits on every `;`, so `\e]9;4;3;50\a` arrives as `["9","4","3","50"]`. The code rejoins `params[1..]`, which is correct under either shape. (2) herdr's retention is **not** a model to copy verbatim: it stores every OSC 9 payload, so a ConEmu cwd report (`9;9;<path>`) or an iTerm2 notification (`9;<text>`) silently overwrites a live progress level with a string no rule matches. Aleph retains only `4`/`4;…`, deliberately. |

### A2 · Scroll regions — DECSTBM (`CSI r`), SU/SD (`CSI S` / `CSI T`), RI (`ESC M`) ✅ SHIPPED 2026-09-04

| | |
|---|---|
| **What herdr does** | libghostty carries a `scrolling_region: ScrollingRegion` on the terminal (`vendor/libghostty-vt/src/terminal/Terminal.zig:65`, initialised `:291`) with top/bottom **and** left/right margins, honoured throughout scrolling and insert/delete (`Terminal.zig:631-639`, `:896-899`). `ESC M` (reverse index) at `vendor/…/src/terminal/stream.zig:2535`; `CSI S`/`CSI T` are in the same CSI table. |
| **What Aleph does today** | **No branch for any of them.** `CSI r`, `CSI S`, `CSI T` all fall into the CSI catch-all at `perform.rs:404`; `ESC M` falls into the ESC catch-all at `perform.rs:351`. `Grid` has no scroll-region field at all (`grid.rs:79-95`), and `Grid::newline` (`grid.rs:523-529`) unconditionally scrolls the **whole** screen via `scroll_up` (`grid.rs:596-609`), which unconditionally evicts row 0 into scrollback. |
| **What breaks in practice** | A program that pins a header or status bar and scrolls only the middle — `tmux`, `less`, `vim`, anything using `CSI 2;23r` — makes Aleph scroll the pinned rows too. The header marches upward off the screen and the agent's own status line (exactly what the manifests match on) disappears from `visible_text()` while it is still visibly on the user's real terminal. `ESC M` is worse in the other direction: a program scrolling *backwards* at row 0 gets nothing, so the top of the screen keeps stale text forever while the program believes it revealed new content. Both produce a `visible_text()` that no longer corresponds to any frame the agent ever painted, which is the failure mode where a wrong label is costlier than a missing one. |
| **Cost to extend** | **Medium.** New `(u16, u16)` region state on `Grid`, defaulting to the full height, plus `set_scroll_region` reset on resize and on RIS. Then `newline`, `scroll_up`, `insert_lines`, `delete_lines` and `erase_in_display` must all read the region instead of `0..rows` — five call sites, each of which currently hardcodes the full-screen assumption. `ESC M` needs a matching `scroll_down` that Grid does not have. Left/right margins (DECLRMM, mode 69) can be left out; almost nothing emits them. One subtlety worth pricing in: when a region is active, rows scrolling off the region top must **not** enter scrollback — only rows leaving a full-height screen do. `scroll_up` currently pushes unconditionally at `grid.rs:601`. |
| **SHIPPED** | 2026-09-04 (terminal round 2, commit `e7cb7e8e8`). `Grid.scroll_region` + `set_scroll_region`; DECSTBM homes the cursor as DEC specifies; `CSI S` / `CSI T` / `ESC M` all read the region, and **all five** of `newline` / `scroll_up` / `insert_lines` / `delete_lines` / `erase_in_display` were moved onto it. Rows leaving a region top do **not** enter scrollback. `resize` and RIS reset it. The three arms guard on an EMPTY intermediate because each has a live `?`-prefixed homonym (`CSI ? r` restores private modes, `CSI ? S` is XTSMGRAPHICS). Guards: `decstbm_scrolls_only_the_region_and_pins_the_header`, `rows_leaving_a_region_top_do_not_enter_scrollback`, `reverse_index_at_region_top_scrolls_down`, `su_and_sd_scroll_within_the_region`, `resize_and_ris_reset_the_region`, `insert_and_delete_lines_respect_the_region`, `erase_in_display_within_a_region_is_still_screen_absolute`. |

### A3 · DECAWM autowrap (`CSI ?7 h` / `CSI ?7 l`) ✅ SHIPPED 2026-09-04

| | |
|---|---|
| **What herdr does** | Mode 7 `wraparound`, `.default = true`, tracked in the mode table at `vendor/libghostty-vt/src/terminal/modes.zig:264`. |
| **What Aleph does today** | **No branch.** `?7h`/`?7l` fail the guard at `perform.rs:401` (which tests `flat.first() == Some(&1049)`) and fall to `perform.rs:404`. `Grid::put` wraps unconditionally at `grid.rs:225-228`. |
| **What breaks in practice** | A program that disables autowrap to paint a full-width status line — the standard trick for writing to the last cell without scrolling — gets a phantom line break from Aleph, and if the cursor was on the bottom row, a phantom **scroll**. Every subsequent frame is then offset by one row against what the program thinks it painted, and because the program never repaints the rows it believes are untouched, the offset persists and compounds. A `bottom_non_empty_lines(8)` region (the shape `qwen.toml:96` and most manifests use) then samples the wrong eight lines. |
| **Cost to extend** | **Small.** One `bool` on `ScreenState` (or `Grid`), one guarded arm alongside the 1049 arm, and one branch in `put`: with wrap off, clamp `cursor_col` at `cols - 1` and overwrite in place instead of calling `newline()`. The `cursor_col <= cols` "wrap is owed" invariant documented at `grid.rs:80-88` is the thing to be careful with — with DECAWM off, that state must never be entered. |
| **SHIPPED** | 2026-09-04 (commit `b7392f4e7`). `Grid.autowrap` via `CSI ?7 h/l`. With autowrap off, `put` overwrites the last column and does **not** enter the deferred-wrap state. Guard: `decawm_off_overwrites_the_last_column_instead_of_wrapping`. |

### A4 · Full reset — RIS (`ESC c`) and DECSTR (`CSI ! p`) ✅ SHIPPED 2026-09-04

| | |
|---|---|
| **What herdr does** | `ESC c` at `vendor/libghostty-vt/src/terminal/stream.zig:2593`, `CSI ! p` (DECSTR) at `stream.zig:1589`. |
| **What Aleph does today** | **No branch for either.** `ESC c` → ESC catch-all `perform.rs:351`. `CSI ! p` carries intermediate `!`, and since `csi_dispatch` only inspects `inter` inside the 1049 arm, action `'p'` reaches the catch-all at `perform.rs:404`. |
| **What breaks in practice** | The most direct instance of "a stale answer read as a current one". An agent that crashes and whose wrapper runs `reset`, or a TUI that issues RIS on exit, leaves Aleph's grid holding the agent's **last painted frame forever**. The manifest engine keeps matching that frame; the runtime table keeps publishing `Working` on the strength of a spinner that stopped spinning minutes ago. Unlike a missing sequence that merely degrades, this one actively manufactures false evidence, and there is no timeout in `screen/` that ever clears it — `RuntimeAgentTable`'s idle hold (`gateway/runtime/mod.rs:191-210`) only bounds working→idle, not evidence staleness. |
| **Cost to extend** | **Small.** `ESC c`: reset grid to blank, cursor home, clear SGR state, drop `saved_cursor`, exit alt screen, reset scroll region and DECAWM, clear the title. `CSI ! p` is the soft variant (same minus the erase and minus scrollback). The only design call is whether RIS should also clear scrollback — xterm does; keeping it is defensible for Aleph since scrollback is not part of `visible_text()`. Say which and write it in the comment. |
| **SHIPPED** | 2026-09-04 (commits `b7392f4e7`, `a1a1e0f78`). `ESC c` (RIS) via `Performer::full_reset`, `CSI ! p` (DECSTR) via `soft_reset` — the `!` arrives as an INTERMEDIATE, which is the whole of what separates it from an unrelated `CSI p`. RIS clears grid, title, saved cursor, the retained OSC 9;4 progress level and both mode bits; **scrollback survives on purpose** (`Grid::reset` says why). A cleared title reaches the wire as `Some("")` (`published_clear`), because a `.flatten()` would have made Some→None read as "unchanged". Guards: `ris_clears_grid_title_and_saved_cursor`, `decstr_resets_modes_but_keeps_the_grid`, `ris_clears_the_progress_level_and_the_two_mode_bits`, `decstr_restores_the_two_mode_bits_but_keeps_the_progress_level`, `a_title_cleared_by_ris_reaches_the_wire_as_an_empty_string`. ⚠️ Deliberate divergence recorded rather than hidden: this DECSTR also exits the alternate screen, which xterm's soft reset does not. |

### A5 · IND (`ESC D`) and NEL (`ESC E`) ✅ SHIPPED 2026-09-04

| | |
|---|---|
| **What herdr does** | `ESC D` (index — line feed without CR) at `vendor/libghostty-vt/src/terminal/stream.zig:2505`; `ESC E` (next line — CR + LF) at `stream.zig:2515`. |
| **What Aleph does today** | **No branch.** Both hit the ESC catch-all at `perform.rs:351`. `esc_dispatch` handles only `7` and `8` (`perform.rs:326-350`). |
| **What breaks in practice** | Text lands on the wrong row. A program emitting `ESC D` expects the cursor one row down at the same column; Aleph leaves it where it was, so the next run of characters overwrites the line above instead of starting a new one. Output is not lost, it is *overlaid* — which is worse than lost, because the resulting line is a plausible-looking mixture of two real lines and a manifest regex can match it. Likelihood is moderate: most output uses `\n`, but terminfo-driven `cursor_down`/`newline` capabilities resolve to these on many entries. |
| **Cost to extend** | **Small.** Two arms in the existing `match byte` in `esc_dispatch`: `b'D' => grid.newline()`, `b'E' => { grid.carriage_return(); grid.newline(); }`. Both grid methods already exist (`grid.rs:523`, `:531`). Once A2 lands, `ESC D` must respect the scroll region — same call path as `\n`, so it follows for free. |
| **SHIPPED** | 2026-09-04 (commit `b7392f4e7`). `ESC D` (IND) is `newline()` — **not** a carriage return as well, which would overlay the next run of text onto the start of the line and produce a plausible mixture of two real lines that a manifest regex can match. `ESC E` (NEL) is carriage return + newline. Guard: `ind_moves_down_same_column_nel_moves_down_col_zero`. |

### A6 · REP (`CSI Ps b`) — repeat preceding character ✅ SHIPPED 2026-09-04

| | |
|---|---|
| **What herdr does** | Handled at `vendor/libghostty-vt/src/terminal/stream.zig:1571-1572` ("Repeat Previous Char (REP)"). |
| **What Aleph does today** | **No branch** — CSI catch-all at `perform.rs:404`. Aleph also does not retain a "last printed char", so there is nothing to repeat even if the arm existed. |
| **What breaks in practice** | A horizontal rule emitted as `─` + `CSI 79 b` renders as a single `─` followed by 79 missing columns. Everything after it on that row is shifted 79 columns left. Any manifest rule anchored with `^` or matching a box-drawn separator sees a line that does not exist on the real screen. Likelihood is genuinely uncertain: `rep` is in modern terminfo and libraries that consult terminfo (ncurses, tmux) will emit it, while hand-rolled TUI renderers (ratatui, Ink) do not. I did not verify which of Aleph's 21 target agents emit it. |
| **Cost to extend** | **Small.** One `char` on `ScreenState` set in `print`, one arm calling `put` N times. The only trap is that REP must repeat the last *printed* character, not the last character seen — a control byte or escape between the two invalidates it, so the field must be cleared in `execute` and on CSI dispatch. |
| **SHIPPED** | 2026-09-04 (commit `b7392f4e7`). `CSI Ps b` over `Grid.last_printed`; a missing candidate writes **nothing** (the alternative is repeating whatever byte came last). Every C0 control and every non-CSI escape invalidates the candidate unconditionally — including bytes no arm claims, because "the dispatcher ignored it" is not the same as "it never arrived". Guard: `rep_repeats_the_last_printed_char_and_a_control_byte_invalidates_it`. |

### A7 · DEC Special Graphics charset — SCS (`ESC ( 0`, `ESC ) 0`, and `SI`/`SO`)

| | |
|---|---|
| **What herdr does** | libghostty has a full charset layer (`vendor/libghostty-vt/src/terminal/charsets.zig`), wired at `vendor/…/src/terminal/stream.zig:2451-2459` (`configureCharset` for `ascii`, `british`, `dec_special`). |
| **What Aleph does today** | **Explicitly discarded, not merely unbranched** — and this is the one place the distinction matters. `esc_dispatch` early-returns on any non-empty intermediates at `perform.rs:323-325`; `ESC ( 0` arrives as `intermediates = [b'(']`, `byte = b'0'`, so it is thrown away *before* the `match`. The comment above it (`perform.rs:320-322`) explains the guard as protecting against mis-reading DECALN, which it does correctly — the charset loss is collateral, not a decision. `SI`/`SO` (0x0F/0x0E) additionally hit the `execute` catch-all at `perform.rs:311`. |
| **What breaks in practice** | Direct text corruption of the readable kind: with G0 mapped to DEC Special Graphics, the program sends `lqqqk` meaning `┌───┐`, and Aleph's `visible_text()` returns the literal `lqqqk`. A model reading that screen sees garbage words interleaved with real ones, and a manifest `line_regex` anchored on a box character never matches. **The trigger is conditional**, which is why this sits at the bottom of Tier A rather than the top: modern agent TUIs emit UTF-8 box characters directly. The exposure is ncurses-derived programs running under a non-UTF-8 locale or with `NCURSES_NO_UTF8_ACS` set, `dialog`/`whiptail`, and `tmux` in ACS mode. I did not measure how often Aleph's actual workloads hit it. |
| **Cost to extend** | **Medium.** Needs a G0/G1 slot pair plus a shift state on `Screen`, `SI`/`SO` arms in `execute`, an `ESC ( ` / `ESC ) ` path that survives the intermediates guard, and a 31-entry lookup applied in `print` before `Grid::put`. Medium rather than small because the intermediates early-return at `perform.rs:323` has to be narrowed carefully — it is currently load-bearing for DECALN and the comment says so; loosening it without replacing that protection re-opens the bug it was written for. |

### A8 · Legacy alternate screen — modes 47 and 1047 ✅ SHIPPED 2026-09-04

| | |
|---|---|
| **What herdr does** | `alt_screen_legacy` = 47 and `alt_screen` = 1047 are distinct tracked modes alongside 1049 (`vendor/libghostty-vt/src/terminal/modes.zig:271`, `:292`, `:294`); `save_cursor` = 1048 at `:293`. |
| **What Aleph does today** | **No branch.** The arm at `perform.rs:401` is guarded on `flat.first() == Some(&1049)`; 47 and 1047 fail the guard and fall to `perform.rs:404`. `Screen::alt_screen()` (`perform.rs:82-84`) is defined as `saved.is_some()`, so it reports `false` throughout. |
| **What breaks in practice** | A program using the legacy pair paints its full-screen UI **onto the primary grid**, and on exit does not restore — so the shell's scrollback is destroyed and the agent's last frame stays on screen indefinitely. Simultaneously the `alt_screen` flag published to the Panel and to `PtyScreenPatch` (`convert.rs:44`) says `false` while a full-screen program is plainly running. Likelihood is low — 1049 has been the default for two decades — but the severity is the same as A4, and the diagnosis cost is high because nothing looks broken until you notice the scrollback is gone. |
| **Cost to extend** | **Small.** Widen the guard at `perform.rs:401` from `Some(&1049)` to the set `{47, 1047, 1049}`, and route 1048 to the existing `saved_cursor` slot. The one real semantic difference: 1049 clears the alt grid on entry and restores the cursor on exit; 47/1047 do neither. `toggle_alt_screen` (`perform.rs:246-289`) already creates a fresh `Grid` on entry, which is 1049's behaviour, so 47/1047 would need the incoming grid preserved across enter/exit rather than recreated. Price that as the actual work; the guard widening is trivial and, done alone, would be wrong. |
| **SHIPPED** | 2026-09-04 (commits `b7392f4e7`, `a1a1e0f78`). Modes `?47` / `?1047` reuse the alternate buffer and **keep** what was parked there on the last exit; `?1048` saves/restores the cursor; `?1049` = 1047 + 1048 per xterm, so it saves and restores cursor and SGR and always starts from a cleared alt grid. The parked buffer follows `resize` and the scrollback limit. Guards: `legacy_alt_screen_47_and_1047_keep_the_alt_grid_across_exit`, `mode_1048_saves_and_restores_the_cursor`, `mode_1049_saves_and_restores_the_cursor_and_style`, `a_parked_alternate_buffer_follows_resize_and_the_scrollback_limit`. ⚠️ Known residue: RIS leaves the parked alt buffer populated, so a `?47` entry after a reset restores pre-reset content. |

---
## Tier B — Marginal: real, bounded, and not on the path that decides a state

### B1 · Live working directory — OSC 7, OSC 9;9, OSC 1337 `CurrentDir=` ⚠️ PARTIALLY SHIPPED 2026-09-04 (OSC 7 only)

| | |
|---|---|
| **What herdr does** | libghostty parses all three into a pwd-change queue (`report_pwd` in the OSC command union, `vendor/libghostty-vt/src/terminal/osc.zig:61`). herdr drains it every write and keeps the last one: `src/pane/terminal.rs:1309-1314` (`take_pwd_changes().filter_map(parse_reported_cwd).next_back()`), with URI/percent/UNC decoding in `src/pane/osc.rs:317-320` and `:630-660`. The round trip is covered at `src/pane/terminal.rs:3807` (`\e]9;9;/tmp/conemu`, `\e]1337;CurrentDir=/tmp/iterm2`). |
| **What Aleph does today** | **No branch** — OSC 7/9/1337 all fall out the bottom of `osc_dispatch` at `perform.rs:411-415`. |
| **What breaks in practice** | Nothing in *classification* — I checked, and no manifest uses a `cwd` region (`crates/agent-detect/src/manifest.rs` routes only `screen`, `osc_title`, `osc_progress` and the line regions). What breaks is *display*: `RuntimeAgentEntry.cwd` reaches both panels, and its own doc comment at `shared/protocol/src/runtime.rs:39-42` says it is the spawn directory, not the live one. A session that `cd`s shows a stale path forever. The reason this is a finding and not a restatement: **that doc comment names PID probing as the remedy, and PID probing is the more expensive of the two options.** herdr does not probe PIDs for cwd; it reads OSC 7. Wiring that lands entirely inside `screen/` and touches no platform API, so it does not carry the R1 decision the comment anticipates. |
| **Cost to extend** | **Small** for the VT half — one arm in `osc_dispatch`, one `Option<String>` on `ScreenState`, one accessor, and a `cwd` source swap at `gateway/runtime/mod.rs:147`. Note the coverage limit honestly when writing it up: OSC 7 only arrives from shells with integration configured, so it is a *supplement* to a spawn-dir fallback, not a replacement — and the two remedies are complementary, not competing (PID probing still covers shells that emit nothing). |
| **SHIPPED (partial)** | 2026-09-04 (commit `e04ca0898` for the VT half, `734af02e0` for the consumer). **OSC 7 only.** `ScreenState.cwd` + `Screen::cwd()`; a `file://` URI is accepted only with an EMPTY host or `localhost` (a path on another machine is a specific lie about where the session is), and percent-decoded. This row's own prediction held: the consumer is a three-tier order derived in ONE place (`pty/manager.rs::flush_session`) — OSC 7 › the foreground process's own cwd (the probe from the same round) › the spawn dir — so the two remedies are complementary, not competing. **NOT shipped: OSC 9;9 (ConEmu) and OSC 1337 `CurrentDir=` (iTerm2).** `osc_dispatch` retains only `9;4` out of the OSC 9 namespace and has no 1337 arm. Guards: `osc7_file_uri_with_empty_or_localhost_host_sets_cwd_and_percent_decodes`, `osc7_with_a_foreign_host_is_dropped`, `cwd_prefers_osc7_then_foreground_then_spawn`. |

### B2 · Combining marks, ZWJ and variation selectors

| | |
|---|---|
| **What herdr does** | libghostty stores grapheme clusters per cell and has a `grapheme_cluster` mode (2027, `vendor/libghostty-vt/src/terminal/modes.zig:297`); herdr reassembles a cell's codepoints into text at `src/pane/terminal.rs:990` (`terminal_cell_text(graphemes: &[u32])`). |
| **What Aleph does today** | `Grid::put` **explicitly drops** every zero-width character: `grid.rs:210-213` computes `UnicodeWidthChar::width(c).unwrap_or(0)` and returns early on `width == 0`. That covers combining accents, ZWJ (U+200D) and VS16 (U+FE0F). |
| **What breaks in practice** | Decomposed text loses its accents in `visible_text()` (`e` + U+0301 → `e`), and multi-codepoint emoji collapse to their base (`👨‍👩‍👧` → `👨`). For *rendering* in the Panel this is a visible defect. For *classification* the effect is ambiguous and I want to be honest that I did not resolve it: dropping VS16 can equally well **help** a manifest anchored on a bare codepoint — `claude.toml:219`'s `^\x{2733} ` requires U+2733 immediately followed by a space, which a VS16-preserving emulator would fail and Aleph passes. So this is not straightforwardly a regression against herdr on the detection path; it is a fidelity gap that happens to cut both ways. I did not test which behaviour the 21 ported manifests were authored against. |
| **Cost to extend** | **Medium.** `Cell.ch` is a single `char` by design (`grid.rs:42-48`, sized deliberately — "a 1000-line scrollback at 200 columns is 200k cells", `grid.rs:17-18`). Holding clusters means either a side table keyed by cell index or a `SmallVec`, and it ripples into `row_text`, `diff::StyleRun` and the wire types in `convert.rs`. Not worth doing for the display defect alone; worth revisiting only if a manifest is shown to need it. |

### B3 · Programmable tab stops — HTS (`ESC H`), TBC (`CSI g`)

| | |
|---|---|
| **What herdr does** | libghostty carries a `Tabstops` structure (`vendor/libghostty-vt/src/terminal/Tabstops.zig`). |
| **What Aleph does today** | Explicitly not modelled, and the code says so: `const TAB_WIDTH: u16 = 8` with the comment "HTS/TBC (programmable stops) are not modelled, so every stop is a multiple of this" (`grid.rs:71-74`). `ESC H` hits the ESC catch-all (`perform.rs:351`), `CSI g` the CSI catch-all (`perform.rs:404`). |
| **What breaks in practice** | Columns misalign for a program that sets custom stops and then relies on `\t` — the text is all present, just at wrong columns. Manifest rules that use `line_regex` with `\s+` (the common shape, e.g. `grok.toml:87`) are insensitive to it. Low likelihood: almost nothing sets custom stops any more. |
| **Cost to extend** | **Small–medium.** A `Vec<bool>` of stops on `Grid`, reset on resize, plus two arms. Only worth doing if something concrete demonstrates the misalignment. |

### B4 · Insert/replace mode — IRM (`CSI 4 h` / `CSI 4 l`)

| | |
|---|---|
| **What herdr does** | ANSI mode 4 `insert`, `vendor/libghostty-vt/src/terminal/modes.zig:254`. |
| **What Aleph does today** | **No branch.** Non-private `h`/`l` fail the guard at `perform.rs:401` (which requires `inter == b"?"`) and fall to `perform.rs:404`. `Grid::put` always overwrites. |
| **What breaks in practice** | A line editor in insert mode gets overwrite semantics, so typed-into-the-middle text clobbers what follows instead of shifting it. Confined to the row being edited and self-corrects on the next full redraw. Very low likelihood: modern line editors use `CSI @` (ICH) — which Aleph **does** handle (`perform.rs:377`) — rather than IRM. |
| **Cost to extend** | **Small.** One flag, one branch in `put` delegating to the existing `insert_chars` (`grid.rs:375`). |

### B5 · Origin mode — DECOM (`CSI ?6 h`) ✅ SHIPPED 2026-09-04 (with A2, as this row required)

| | |
|---|---|
| **What herdr does** | Mode 6 `origin`, `vendor/libghostty-vt/src/terminal/modes.zig:263`. |
| **What Aleph does today** | **No branch** (`perform.rs:404`); `Grid::goto` (`grid.rs:289`) is always screen-absolute. |
| **What breaks in practice** | Nothing *today*, because DECOM only means anything when a scroll region is set and Aleph has none. It becomes a real defect the moment A2 lands: a program that sets a region, enables origin mode, and then issues `CSI 1;1H` expects the top of the *region*, and would get the top of the screen. Listed here so it is not forgotten — **A2 and B5 must ship together or A2 is half-right.** |
| **Cost to extend** | **Small**, given A2. One flag plus a row offset inside `goto`/`goto_row`. Zero value before A2. |
| **SHIPPED** | 2026-09-04 (commit `e7cb7e8e8`, the same commit as A2 — "A2 and B5 must ship together or A2 is half-right" was followed). `Grid.origin_mode` via `CSI ?6 h/l`; `goto` and `goto_row` add the region offset. Guard: `origin_mode_makes_cup_relative_to_the_region`. |

### B6 · Selective erase — DECSED / DECSEL (`CSI ? J`, `CSI ? K`)

| | |
|---|---|
| **What herdr does** | Distinguished in libghostty; `selective_erase` is a declared DA1 feature (`vendor/libghostty-vt/src/terminal/device_attributes.zig:50`). |
| **What Aleph does today** | Aleph's `J`/`K` arms (`perform.rs:387-396`) read `flat.first()` and **never inspect `inter`**, so `CSI ? 2 J` is executed as `CSI 2 J` — i.e. treated as an unconditional erase. This is one of the few places Aleph silently *widens* a sequence rather than dropping it. |
| **What breaks in practice** | Cells the program marked non-erasable (DECSCA) get erased. Almost nothing uses DECSCA, so in practice the widened reading is the same as the correct one, and where it differs it errs toward a blanker screen — which reads as "less evidence", never as false evidence. Benign under Aleph's fail-closed direction. |
| **Cost to extend** | **Small**, and probably not worth it. Would need a per-cell "protected" bit, which costs more in `Cell` size than the defect costs. |

### B7 · SGR breadth — faint, blink, conceal, strikethrough, overline, underline styles and colour

| | |
|---|---|
| **What herdr does** | libghostty's SGR enum covers `faint` (2), `blink` (5/6), `invisible` (8), `strikethrough` (9), resets 25/28/29, `overline`/`reset_overline` (53/55), and `underline_color` / `256_underline_color` / `reset_underline_color` (58/59) — `vendor/libghostty-vt/src/terminal/sgr.zig:27-54`, dispatched at `sgr.zig:240-415`. Underline *styles* (`4:1`–`4:5`) are in the same table. |
| **What Aleph does today** | `Attrs` is one byte with exactly four flags — BOLD, ITALIC, UNDERLINE, REVERSE (`grid.rs:22-26`), deliberately, so `Cell` stays small (`grid.rs:17-18`). The `sgr` match at `perform.rs:187-240` handles 0/1/3/4/7/22/23/24/27, 30–37/39, 40–47/49, 90–97, 100–107, and 38/48 with both `;` and `:` forms. Everything else hits `perform.rs:239` (`_ => {}`). Colour coverage is therefore **complete** — 16, 256 and truecolour all land. |
| **What breaks in practice** | Purely rendering fidelity in the Panel: dim placeholder text renders at full intensity, strikethrough and blink are lost. `visible_text()` is unaffected — every one of these is an attribute, not a character. One asymmetry worth naming rather than burying: SGR 8 (conceal) is ignored, so text the real terminal would hide is present in `visible_text()`. For a model reading the screen that is arguably the more useful behaviour, but it is a behaviour difference and should be a stated choice rather than an accident of the missing arm. |
| **Cost to extend** | **Medium**, and gated on a decision rather than on effort. `Attrs(u8)` is full at four flags; adding faint/blink/conceal/strike/overline means widening to `u16`, which changes `PtyAttrs` on the wire (`convert.rs:22-24` passes the raw byte through) and therefore is a cross-crate contract change touching `shared/protocol` and the Panel decoder. Underline *colour* is a second colour field per cell — that one is genuinely large and not worth it. |

### B8 · OSC 8 hyperlinks

| | |
|---|---|
| **What herdr does** | libghostty emits `hyperlink_start` / `hyperlink_end` commands (`vendor/libghostty-vt/src/terminal/osc.zig:100-107`, parsed at `osc.zig:801`); herdr reads back per-cell link targets for the visible area at `src/pane/terminal.rs:2068` and `:494` (`visible_hyperlinks`), so a human can click them. |
| **What Aleph does today** | **No branch** — OSC 8 falls out the bottom of `osc_dispatch` (`perform.rs:411-415`). |
| **What breaks in practice** | Only the URL is lost. The *visible* text of a hyperlink is ordinary printable output between two OSC sequences, so `vte` delivers it to `print()` normally and it lands on the grid intact — `visible_text()` is byte-for-byte correct. herdr needs this because a human clicks links in a rendered pane; Aleph publishes text to a model. Genuinely no cost to the stated use case. |
| **Cost to extend** | **Large**, and there is no reason to. Per-cell link identity needs a ref-counted URL set plus a per-cell id, which is exactly the `Cell` size problem from B2 and B7 again. |

---
## Tier C — Irrelevant to Aleph's use case: herdr needs these because it paints for humans and forwards their keystrokes

These are real capability differences and would all be gaps if Aleph were a terminal. It is not: it samples
text and a title on the server and publishes a state. Each row says why the difference costs nothing.

### C1 · DCS hook / put / unhook — the named hypothesis, and it costs almost nothing ✅ SHIPPED 2026-09-04

The plan named this as a known gap. **Confirmed as a gap, refuted as a cost.**

`Screen` does not implement `hook`, `put` or `unhook`, so `vte::Perform`'s defaults apply — silent no-ops.
But `vte` runs the whole DCS state machine itself: `action_hook` transitions to `State::DcsPassthrough`
(`vte-0.14.1/src/lib.rs:465-473`), and `advance_dcs_passthrough` (`lib.rs:317-337`) routes payload bytes to
`performer.put()` and **never to `print()`**. So a DCS payload is *swallowed by the parser*, not painted onto
the grid. Nothing corrupts. Aleph also never writes back to the PTY from `screen/`, so the two things
libghostty's DCS handler is actually for — XTGETTCAP replies (`vendor/libghostty-vt/src/terminal/dcs.zig:81-91`)
and DECRQSS replies (`dcs.zig:96-102`) — have no consumer here either.

Two narrow caveats, stated so the "costs nothing" is not overclaimed:

- **Unterminated DCS swallows following text until the next ESC.** `advance_dcs_passthrough` leaves the state
  only on `ESC`, `CAN`, `SUB` or `ST` (`lib.rs:320-334`). A truncated sixel or a program killed mid-DCS makes
  the next run of plain output vanish from the grid until any escape sequence appears. Bounded (any escape
  breaks out, and TUIs emit them constantly) but not zero.
- Implementing `hook`/`put`/`unhook` as **explicit** no-ops with a comment would be worth doing on its own,
  precisely so the next reader knows the silence is a decision rather than the same by-construction omission
  that `perform.rs:317-319` records for `esc_dispatch`. That is a 6-line change, not a capability.

**Cost to extend: small, and the reason to do it is documentation, not behaviour.**


**SHIPPED 2026-09-04** (commit `b7392f4e7`). `hook` / `put` / `unhook` are now explicit no-ops with a comment — exactly the 6-line documentation change this row argued for, so the silence is a decision rather than the by-construction omission. Guard: `dcs_hook_put_unhook_are_explicit_no_ops`. The unterminated-DCS caveat above still stands: that is `vte`'s state machine, not Aleph's.
### C2 · Kitty graphics (APC)

herdr wires it end to end: libghostty's APC handler (`vendor/libghostty-vt/src/terminal/apc.zig:13-29`) with a
whole `kitty/graphics_*.zig` family behind it, surfaced as placements at `src/pane/terminal.rs:2076-2092` and
composited by `src/kitty_graphics.rs`. Aleph has no branch: `vte` routes APC to `State::SosPmApcString` →
`anywhere` (`vte-0.14.1/src/lib.rs:181`, `:438-450`), which discards everything up to the terminator without
reaching `print()`. **Nothing breaks** — an image is not text, `visible_text()` is unaffected, and the payload
does not leak onto the grid. Cost to extend would be **large** (an image store, a placement model, a wire
format, a Panel renderer) for zero classification value.

### C3 · Sixel — neither side has it

Worth a row precisely because it is easy to assume herdr does. It does not: the vendored libghostty has **no
sixel implementation**, only the DA1 feature *code* in an enum (`vendor/libghostty-vt/src/terminal/device_attributes.zig:53`),
and the default advertised feature set is `&.{.ansi_color}` (`device_attributes.zig:37`) — sixel is not
advertised. Aleph swallows sixel as DCS (C1). **No gap in either direction.**

### C4 · Kitty keyboard protocol (`CSI > … u`, `CSI = … u`)

herdr tracks the flag stack in its own Rust scanner over the PTY stream — `KittyKeyboardTracker`
(`src/pane/kitty_keyboard.rs:1-8`, `observe` at `:11-60`) — and uses it to encode a human's keypresses back
(`src/pane/terminal.rs:1663`, `:1849` `encode_terminal_key`). Aleph has no branch (`perform.rs:404`). **Nothing
breaks for detection**: this is entirely about *input* encoding. It becomes relevant only if the Panel's
terminal view wants disambiguated keys — and note the structural point if that day comes: the mode-set
sequence is only ever seen by the *server*, so the flag would have to be tracked in `screen/` and published,
even though the encoding itself belongs in the Panel's `keymap.rs`. Cost then: **medium**.

### C5 · Bracketed paste (mode 2004) ✅ SHIPPED 2026-09-04

herdr tracks it (`vendor/libghostty-vt/src/terminal/modes.zig:295`) and exposes
`bracketed_paste_enabled()` (`src/pane/terminal.rs:1689`) so a paste is wrapped in `\e[200~ … \e[201~`.
Aleph has no branch. **Nothing breaks for detection.** For the Panel it is latent: the terminal view at
`interfaces/webchat/src/platform/wide/views/terminal/` has a `keymap.rs` but no paste path, so there is no
consumer to starve today. Same structural note as C4 — the mode bit can only be observed server-side.
Cost: **small** (one flag, one accessor, one wire field) if a paste path is ever added.


**SHIPPED 2026-09-04** (VT half `e04ca0898`, consumer `5422d0aff`). `ScreenState.bracketed_paste` via `CSI ?2004 h/l`, published on `PtyScreenPatch.bracketed_paste` (`Option<bool>`, `Some` only when it changes). The paste path this row said did not exist now does: the Panel canvas's `on:paste` wraps in `ESC[200~ … ESC[201~` **only** when the last patch said the mode is on. `None` means "we have not been told" and takes the weakest assumption — no wrapping. RIS clears it. Guards: `bracketed_paste_mode_rides_the_patch`, `paste_wraps_when_bracketed_paste_is_on_and_not_when_unknown`, `bracketed_paste_starts_unknown_and_only_a_patch_moves_it`.
### C6 · Mouse tracking (modes 1000 / 1002 / 1003 / 1006 / 1016) and focus reporting (1004)

herdr tracks all of them (`vendor/libghostty-vt/src/terminal/modes.zig:279-287`, `:282`), exposes
`mouse_reporting_enabled()` / `sgr_pixel_mouse_enabled()` (`src/pane/terminal.rs:1697`, `:1703`), and encodes
events at `src/pane/terminal.rs:1952`. Aleph has no branch, and the Panel terminal view has no mouse handling
at all. **Nothing breaks.** Cost if wanted later: **medium** (mode set in `screen/`, wire fields, encoder in
the Panel).

### C7 · Synchronized output (mode 2026)

herdr reads it every write (`src/pane/terminal.rs:1327-1330`, accessor `:1837`) to avoid presenting a torn
frame to a human eye. Aleph has no branch. **Nothing breaks, and arguably it should stay that way**: Aleph's
publishing cadence is already a coalescing diff (`Screen::take_patch`, `perform.rs:125-147`, driven by the
flush loop at `gateway/pty/manager.rs:578`), which solves tearing structurally rather than per-frame. Adding
2026 would be a second answer to a question that already has one.

### C8 · Device replies — DA1/DA2 (`CSI c`), DSR (`CSI n`), DECRQSS, XTGETTCAP

herdr answers all of these: DA encoding at `vendor/libghostty-vt/src/terminal/device_attributes.zig:70`,
XTGETTCAP with a dedicated Rust tracker at `src/pane/xtgettcap.rs`, and an ordered response path that
interleaves replies with PTY writes (`src/pane/terminal.rs:1372` `write_pty_bytes_with_ordered_responses`).
Aleph answers none — `screen/` has no write-back channel to the PTY by construction. **Nothing breaks today.**
The honest caveat: a program that *queries* and waits for a reply will time out rather than get a wrong
answer, which is the fail-closed direction, but a program that adapts its output to the reply may fall back to
a dumber rendering than it needed to. I did not find a case where this bites in practice, and I did not test
one. Cost to extend: **large** — it is not a `perform.rs` branch, it is a new outbound edge from `screen/`
to the PTY writer, with ordering guarantees (herdr needed a whole ordered-response mechanism for exactly this).

### C9 · Cursor visibility (`?25`), cursor shape (DECSCUSR `CSI q`), cursor blink (`?12`) ⚠️ PARTIALLY SHIPPED 2026-09-04 (`?25` only)

herdr tracks all three (`vendor/libghostty-vt/src/terminal/modes.zig:267-268`, shape mapping at
`src/pane/terminal.rs:104`, exposed as `TerminalCursorState` at `:84`/`:438`). Aleph has no branch; it
publishes a cursor *position* only (`diff::ScreenPatch.cursor`, `perform.rs:133`). **Nothing breaks for
detection**; the Panel renders a cursor that a program asked to hide. Cost: **small**, cosmetic.


**SHIPPED (partial) 2026-09-04** (VT half `e04ca0898`, renderer `5422d0aff`). **`CSI ?25 h/l` only.** `ScreenState.cursor_visible` rides `PtyScreenPatch.cursor_visible` (`Some` only on change) and the Panel skips drawing the cursor when it is `Some(false)`. RIS/DECSTR reset it to visible. **NOT shipped: DECSCUSR cursor shape (`CSI q`) and blink (`?12`)** — no branch for either. Guards: `cursor_visibility_rides_the_patch_only_when_it_changes`, `cursor_visible_false_is_stored_and_render_skips_the_cursor`.
### C10 · Colour palette and clipboard OSCs — OSC 4 / 10 / 11 / 12 / 104, OSC 52

herdr has a whole tracker for palette queries because it must answer them and reconcile them with the host
terminal's theme (`DefaultColorOscTracker`, `src/pane/osc.rs:39-56`; theme restore at
`src/pane/terminal.rs:1145`). Aleph has no branch, and **should not**: `Color::Default` is deliberately a
*variant* rather than a resolved RGB, because "the server does not know the client's palette"
(`grid.rs:5-8`). Palette state is the client's, not the screen's. OSC 52 is a clipboard write-back with no
channel and no consumer. **No gap; a deliberate architectural difference.**

### C11 · Left/right margins (DECLRMM, mode 69) and rectangular editing

libghostty carries left/right in its scrolling region (`vendor/libghostty-vt/src/terminal/Terminal.zig:631-639`).
Aleph has neither. Effectively nothing emits DECLRMM. **No practical cost**; explicitly out of scope for A2.

---
## What Aleph already handles — the half that makes the table above trustworthy

I checked each of these against `screen/` rather than assuming, because a gap list is only as good as its
false-positive rate, and this project treats a wrong label as costlier than a missing one.

| Capability | Where | Note |
|---|---|---|
| Alternate screen 1049, enter and exit | `perform.rs:401` → `toggle_alt_screen` `perform.rs:246-289` | Handled *more* carefully than a naive impl: the swap is applied inline at parse time, not deferred to the end of the chunk (`perform.rs:247-255` records why the deferred version passed its test and still broke `vim`), a nested `?1049h` is a no-op so the real primary is never clobbered (`:258-268`), and the newly-current grid is force-dirtied so an attached client actually repaints (`:274-288`). |
| OSC 0 / OSC 2 window title | `perform.rs:408-415`, published via `take_patch` `perform.rs:130-131` | Including a title split across two PTY reads — the parser is retained across `feed` calls exactly for this (`perform.rs:57-66`, test `perform.rs:460-466`). |
| SGR colour, complete | `perform.rs:187-241` | 16-colour, bright (90–97/100–107), 256 (`38;5;n`), truecolour (`38;2;r;g;b`) **and** the `:` sub-parameter form (`38:2:r:g:b`), which tmux and terminfo-driven output emit — flattened at `perform.rs:357`. A malformed 38/48 run is skipped rather than mis-parsed into the following parameter. |
| DECSC / DECRC (`ESC 7` / `ESC 8`) | `perform.rs:326-350` | Position **and** style saved together (`SavedCursor`, `perform.rs:27-32`), which is the version that does not silently drop colour on prompts that bracket their output with 7/8. `ESC 8` with nothing saved is a deliberate no-op rather than DEC's home-the-cursor. |
| Wide / CJK double-width glyphs | `grid.rs:209-260` plus `repair_straddled_glyph` `:262-287`, `repair_row_pairs` `:414-432`, `repair_edge_truncated_glyph` `:585-595` | This is the most carefully-built part of the grid: a spacer cell model with orphan repair on overwrite from either side, on row edits, and on narrowing resize. More thorough than most emulators bother with. |
| Cursor movement, full set | `perform.rs:366-374` | CUP/HVP (`H`/`f`), CUU/CUD/CUF/CUB (`A`–`D`), CHA (`G`), VPA (`d`), all 1-based on the wire and clamped (test `perform.rs:521-527`). |
| Erase / insert / delete | `perform.rs:375-396` | ECH (`X`), DCH (`P`), ICH (`@`), IL (`L`), DL (`M`), ED (`J`), EL (`K`), with the correct exception that `J`/`K`'s `0` is a real mode value, not "use default" (`perform.rs:362-364` vs `:387-396`). |
| C0 controls | `perform.rs:302-312` | LF, CR, BS, HT, BEL, plus VT (0x0B) and FF (0x0C) mapped to LF as xterm does. |
| Scrollback with a configurable ceiling | `grid.rs:596-609`, `Screen::set_scrollback_limit` `perform.rs:105-110` | Applied to the saved primary underneath the alt screen too, so a config change during `vim` is not lost on exit. |
| Bell as an edge, not a level | `perform.rs:73-76` | `take_bell` clears on read; test `perform.rs:479-484`. |
| Resize, including under the alt screen | `perform.rs:88-99` | Resizes the stashed primary as well, and force-dirties. Reflow is deliberately not attempted (`grid.rs:535-538` gives the reason). |
| Split escape sequences across read boundaries | `perform.rs:57-66`, test `perform.rs:497-504` | The retained-parser property, tested for both CSI and OSC. |
| DCS and APC payloads do not reach the grid | `vte-0.14.1/src/lib.rs:317-337`, `:181`+`:438-450` | A correctness property, not an accident — see C1/C2. |
| Intermediates are not ignored blindly | `perform.rs:320-325` | The early return exists specifically so `ESC # 8` (DECALN) is not executed as `ESC 8` (DECRC). It is also what discards SCS (A7) — worth knowing before loosening it. |
| The `vte` API surface is pinned | `mod.rs:19-84` | A mutation-verified compile-time guard (changing `execute`'s `byte: u8` to `u32` produces `E0053` naming the method), so a `vte` upgrade that drifts `Perform`'s signatures fails here first rather than as a wall of confusing test failures. |

---

## Things I could not determine, and one number that is wrong

**Could not determine (stated rather than guessed):**

1. **Whether REP (A6) fires for any real Aleph workload.** `rep` is in modern terminfo, so ncurses- and
   tmux-derived output will use it, but I found no way to establish from source which of the 21 target agents
   actually emit it. The severity claim in A6 is sound; the likelihood claim is not measured.
2. **Same for DEC Special Graphics (A7).** The corruption is certain *if* `ESC ( 0` arrives; I could not
   establish how often it does under Aleph's workloads.
3. **Which grapheme behaviour the 21 ported manifests were authored against (B2).** They came from herdr,
   which preserves clusters, but at least one rule (`claude.toml:219`) reads as if authored against a bare
   codepoint. Resolving this needs a real capture, not a reading.
4. **Nothing here was executed.** `cargo` was off-limits for this survey, so every claim is read off source:
   Aleph's `screen/`, herdr's Rust, the vendored libghostty Zig, and `vte` 0.14.1 from the local registry.
   The two claims most worth re-verifying by running something are "vte swallows DCS without printing" (C1)
   and "an unterminated DCS swallows following text until the next ESC" (C1's caveat).
5. **I did not audit herdr's `src/pane/terminal.rs` in full** (6 735 lines). I traced the paths relevant to
   each axis. A capability herdr exposes only through a path I did not trace would not appear above.

**One incidental finding, outside the VT but adjacent enough to report:** the doc comment at
`src/gateway/runtime/mod.rs:114` says "The 29 manifests". There are **21** —
`crates/agent-detect/src/engine.rs:115` declares `SCREEN_MANIFEST_AGENTS: [Self; 21]` and
`manifest.rs:280-302` has 21 `include_str!` entries, matching the 21 files in `manifests/` and herdr's own 21.
A count living in prose next to the array that owns it is the 判据 §1 shape; the array is the authority.

---

## What I would extend first

1. ~~**A1 — OSC 9;4 progress.**~~ **DONE 2026-09-03** — see the ✅ row in A1. The only gap whose consumer already existed and was already asking. One arm in
   `osc_dispatch`, one field, one constant swap at `gateway/runtime/mod.rs:160`; it re-arms four dead manifest
   rules including the highest-priority rule in `grok.toml`.
2. **A4 — RIS / DECSTR.** Cheapest fix with the worst failure mode: without it a crashed agent's last frame
   is republished as current evidence forever, and nothing else in the pipeline can clear it.
3. **A2 + B5 — scroll region (DECSTBM, SU/SD, RI) with origin mode.** The largest single source of
   `visible_text()` that corresponds to no frame the agent ever painted. Medium cost and the only Tier-A item
   that needs new grid state, so it wants planning rather than a branch — and B5 has to ship with it or the
   fix is half-right.

**Deliberately not first:** A3 (DECAWM) is cheap and should be picked up alongside A4, but its trigger is
narrower than A2's. A7 (SCS) is the highest-uncertainty row and should wait for a capture that shows it
firing. Everything in Tier C should be left alone; C1's only worthwhile change is a comment.
