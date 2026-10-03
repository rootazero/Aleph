# Task P8.6 Report — HARNESS_PHILOSOPHY.md §8 第五课 补注 (2026-09-20)

- **Plan**: `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1681-1720`
- **File modified**: `docs/reference/HARNESS_PHILOSOPHY.md` (only this; no code touched)
- **HEAD at start**: `e8bf4d123` clean, `git diff e8bf4d123 -- src/harness/` empty
- **HEAD at end**: `e8bf4d123 + 1 commit` clean after commit (see "Commit" section below)

## Change

Inserted one paragraph immediately after 第五课 and before 第六课 in §8, anchoring on the
existing `**第五课：… 逐项见 FEATURE_LOCATOR §3.1 Round 8。**` paragraph end and the
`**第六课：连续五次「零增删」之后…**` paragraph start. Inserted text is verbatim from the
plan code block (plan lines 1692-1702). The plan code block opens with a blank markdown line,
so the inserted sequence is one blank separator, one new bold paragraph, one blank separator
— total `+2` lines, `0` deletions.

Post-insertion file structure around the insertion point:

```
$ awk 'NR>=346 && NR<=355 {printf "%d|%s\n", NR, $0}' docs/reference/HARNESS_PHILOSOPHY.md
346|---
347|
348|**第五课：…（2026-08-15，deepseek-harness/Cordis）**。…逐项见 FEATURE_LOCATOR §3.1 Round 8。
349|
350|**第五课补注（2026-09-20，插件宿主作用域化轮）：那条「架构本身不移植」的裁定被收窄了一次，收窄的是一句话，不是立场。** …它守的是产地不是覆盖。
351|
352|**第六课：连续五次「零增删」之后…**
```

The new paragraph is on line 350, between the blank-after-第五课 (line 349) and the 第六课
opening (line 352).

## Anchoring rationale (plan line numbers had drifted)

Plan referenced `:350` for the 第五课 paragraph and `:352` for the blank line before 第六课. At
`e8bf4d123`:

- `**第五课：…**` paragraph is at line 348 (one long wrapped line; the trailing `。逐项见 FEATURE_LOCATOR §3.1 Round 8。` is at the end of line 348)
- The blank line after it is at line 349
- `**第六课：…**` paragraph begins at line 350

Per dispatch instruction "按标题/锚文本定位，不按可能过时行号猜测", I anchored on the unique
trailing fragment of 第五课 (`逐项见 FEATURE_LOCATOR §3.1 Round 8。`) followed by a blank
line and the unique opening fragment of 第六课 (`**第六课：连续五次「零增删」之后…`). Both
fragments are unique in the file (`rg -c` returned 1 for each), so the match is unambiguous.
The resulting insertion lands at the semantically correct place (after 第五课, before 第六课),
which is what the plan's `:350` reference was trying to express.

## Pre-condition (plan Step 1)

```
$ rg -n '第五课' docs/reference/HARNESS_PHILOSOPHY.md
348:**第五课：对一个「everything is a plugin」的 harness 做完 10 维对照，落点依旧全在循环之外（2026-08-15，deepseek-harness/Cordis）**。…逐项见 FEATURE_LOCATOR §3.1 Round 8。

$ rg -n '2026-09-20' docs/reference/HARNESS_PHILOSOPHY.md
(no output)
```

- `第五课` → exactly 1 hit (line 348). Plan said "`:350` only"; the deviation is informational —
  the drift is 2 lines (the plan was authored against a slightly older snapshot where the
  blank line and 第五课 paragraph were 2 lines lower). The semantic target (the unique 第五课
  paragraph) is preserved.
- `2026-09-20` → 0 hits. Plan required 0. ✓

## Post-condition (plan Step 3)

```
$ rg -n '第五课补注' docs/reference/HARNESS_PHILOSOPHY.md
350:**第五课补注（2026-09-20，插件宿主作用域化轮）：那条「架构本身不移植」的裁定被收窄了一次，收窄的是一句话，不是立场。** …它守的是产地不是覆盖。

$ rg -n '第六课' docs/reference/HARNESS_PHILOSOPHY.md
352:**第六课：连续五次「零增删」之后，第六次净增 99 行——而真正的教训是关于闸，不是关于行数（2026-08-23，harness 自查轮）**。…
```

- `第五课补注` → exactly 1 hit (line 350). Plan required 1. ✓
- `第六课` → still exactly 1 hit, **after** the new 补注 paragraph (line 352 > line 350). Plan required "still exactly one line, after the new paragraph". ✓

## Validation (per dispatch verification checklist)

- **New heading text** — `**第五课补注（2026-09-20，插件宿主作用域化轮）：…**` matches the plan code block opening verbatim, including the full-width parentheses `（` / `）`, the comma form `，`, the keyword `插件宿主作用域化轮`, and the bold wrapper `**…**`.
- **Key phrases present in the inserted paragraph** (each verified by `rg -n '…' docs/reference/HARNESS_PHILOSOPHY.md`, all return line 350):
  - `Disposer` ✓
  - `EffectScope` ✓
  - `src/harness/\` 增删 0 行` ✓ (with the backtick inside the bold run preserved)
  - `budget.rs::CEILING` ✓
  - `publishing_plugin_projections_has_exactly_one_author` ✓
  - `FEATURE_LOCATOR 附录 D.0.196` ✓
  - `projection.rs:14-24` ✓
  - `DI 容器、Proxy 上下文、级联重启、HMR` (the "仍然不采" enumeration) ✓
- **Section ordering (post-edit)** — the three relevant markers in §8 are now:
  ```
  348: **第五课：…
  350: **第五课补注（2026-09-20，…）…        ← NEW
  352: **第六课：…
  ```
  Fifth → Fifth-supplement → Sixth, in that order. ✓
- **Code fence balance** — `rg -nc '^```' docs/reference/HARNESS_PHILOSOPHY.md` → **4** (unchanged from pre-edit). The new paragraph contains no fenced code block (it's all inline backticked spans); the 4 existing fences (lines 77, 91, 125, 140) are pre-P8.6 and outside the §8 area.
- **Link sanity** — the inserted paragraph introduces no new `[…](…)` link or `http(s)://` URL; the prose reference "FEATURE_LOCATOR 附录 D.0.196" is bare text (the document convention is to cite appendix numbers inline, not as anchored links — see existing 第五课 paragraph's bare "FEATURE_LOCATOR §3.1 Round 8" reference for the same pattern).
- **`git diff --check`** → no whitespace or conflict-marker warnings on the working-tree diff.
- **`git diff e8bf4d123 -- src/harness/`** → 0 lines (R10 / plan "do NOT touch code" honored).
- **`git diff --stat e8bf4d123 docs/reference/HARNESS_PHILOSOPHY.md`** → ` 1 file changed, 2 insertions(+)`.

## Deviations from plan

1. **Plan line anchor `:350` drifted to actual line 348.** The plan was authored against a
   version of `HARNESS_PHILOSOPHY.md` where the 第五课 paragraph sat 2 lines lower than at
   `e8bf4d123`. The dispatch instruction explicitly says "按标题/锚文本定位，不按可能过时行号猜测",
   so I anchored on the unique trailing fragment of 第五课 + the unique opening of 第六课. The
   insertion lands at the semantically correct place (the only place in the file where
   "after 第五课, before 第六课" is true). This is the documented working method, not a
   semantic deviation.

## What was NOT done (state-the-negative, per AGENTS.md §6)

- No source file under `src/` was modified. `git diff e8bf4d123 -- src/harness/` is empty.
- No `cargo fmt`, no `--no-verify`, no `git add -A`, no `git stash`.
- No `~/.claude/settings.json` read; no `~/.claude/.aleph` written; no `aleph-server` started.
- Did **not** advance to P8.7 (FEATURE_LOCATOR.md `§3.10` round entry + new `§5.27` MCP face
  + Appendix D.0.196–199 + Appendix E triggers) or any later task. P8.7 is its own dispatch.
- Did **not** touch `docs/reference/FEATURE_LOCATOR.md`, `docs/reference/EXTENSION_SYSTEM.md`,
  `docs/reference/PLUGIN_SYSTEM.md`, `docs/reference/GATEWAY.md`, `docs/reference/ARCHITECTURE.md`,
  `docs/reference/ALEPH_HUB.md`, `docs/reference/SKILL_MODEL_TAXONOMY.md`, `CLAUDE.md`,
  `qa/README.md`, or `docs/archive/*` slots — those are P8.1–P8.5 / P8.7+ territory.
- Did **not** add a fenced code block, link, or external URL inside the new paragraph.
- Did **not** run `cargo check` / `cargo test` / `cargo clippy` — there is no Rust change to
  validate, and P8.6 is a pure doc task.

## Commit

```
docs(harness-philosophy): 第五课补注 — ownership rule adopted, fiber/DI/cascade still not, harness 0 lines
```

- Conventional-commit form, English, scope `harness-philosophy`.
- Trailer: `Co-Authored-By: MiniMax M3 <noreply@minimax.chat>` (per this dispatch's instruction;
  plan default `Claude Opus 5 (1M context) <noreply@anthropic.com>` not used).
- Files staged explicitly:
  - `docs/reference/HARNESS_PHILOSOPHY.md` (the doc edit)
  - `.superpowers/sdd/2026-09-20-plugin-scope-and-cc-compat/task-P8.6-report.md` (this report,
    added with `git add -f` because `.superpowers/` is root-gitignored)
- `git status` clean after commit.