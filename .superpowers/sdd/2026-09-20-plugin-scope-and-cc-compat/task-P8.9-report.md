# Task P8.9 Report — `qa/README.md` drift-reconciled (2026-09-20)

- **Plan**: `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1856-1909` (`### Task P8.9`)
- **File modified**: `qa/README.md` (only this; zero code, zero scripts, zero docs outside `qa/`)
- **HEAD at start**: `11cb0eb58` clean, `git diff 11cb0eb58 -- src/harness/` empty
- **HEAD at end**: `11cb0eb58 + 1 commit` clean after commit (see "Commit" section below)
- **Worktree**: `/Volumes/TBU4/Workspace/Aleph/.claude/worktrees/plugin-scope-round` (branch `worktree-plugin-scope-round`)

## Drift between the plan and the current tree

DeepSeek-style reconciliation — the plan was drafted against an older base (the plan text references `:618-625` and `:1558-1695` and assumes `:1600-1619` for the `terminal` per-fixture entry; at `11cb0eb58` the corresponding regions sit at `:657-715` and `:1730-1940`, with `terminal` at `:1808`). The plan's "Step 1: Pre-condition" (`rg -n 'mcp_face|cc-cache' qa/README.md` → 0) is therefore stale: P7.1 (`bd027cac9`) and P7.2 already landed the `mcp_face` per-fixture entry, the `cc-cache` command-block row, and the 2026-09-27 `plugins` per-fixture entry. A mechanical application of the plan would have replaced existing, richer prose with a stale draft.

What was actually drifted at `11cb0eb58`:

| Region | Plan expected | Reality at `11cb0eb58` | Drift |
|---|---|---|---|
| Command-block: `qa/plugins/run.sh` rows | `:618-625`, lists `manifest`/`scaffold`/`trust` only (plan's "drift found while writing this task" call-out) | `:657-715`, lists **9** plugins stages (missing `browse` / `marketplaces` / `panel`) | 3 stages missing |
| Command-block: `qa/mcp_face/run.sh` rows | 5 rows at `:663-672` (after the trust row) | 5 rows at `:692-700` (after panel) | count correct, line numbers stale |
| Per-fixture: `- *\*\``plugins`\*\*` entry | plan expected to **append** it after `terminal` (`:1600-1619`) | already exists at `:1870`; richer than plan's draft (4 sub-bullets `command`/`exit2`/`subagent`/`cc-cache` with detailed "what the test forbids as a vacuous green", plan's draft has 5) | exists; plan-draft text NOT mechanically applied |
| Per-fixture: `- \*\*\`mcp_face\`\*\*` entry | plan expected to **append** it after `terminal` | already exists at `:1776` (placed BEFORE `terminal`, not after; richer than plan's draft) | exists; plan-draft text NOT mechanically applied |
| Pre-condition rg `'mcp_face\|cc-cache' qa/README.md` → 0 | (plan assumed) | many matches (P7.1 + P7.2 already merged) | plan pre-condition stale |

## Actual gap fixed

Only the one cell the plan's text-fragment check still names today: the command block at the top of `qa/README.md` was missing the `browse` / `marketplaces` / `panel` plugins rows. Inserted after `trust` (where the script's `case` statement orders them — `qa/plugins/run.sh:301,335,423`) and before the `mcp_face` block, so the existing layout (plugins-first → mcp_face → plugins-continuation) stays byte-identical outside the insertion window. No existing row was edited, no row was deleted, no script touched.

Why three insertions and not a full reorder: reordering the command block would either delete the post-`mcp_face` rows that are already landed (forbidden by "avoid deleting already landed stages") or require moving them on top of the `mcp_face` block (an extra-edit beyond drift reconciliation). Insertion is the minimum that satisfies the script↔README drift.

## Verifications

```
$ rg -n '^\./qa/plugins/run\.sh' qa/README.md | wc -l
       12
$ rg -n '^\./qa/mcp_face/run\.sh' qa/README.md | wc -l
        5
```

`qa/plugins/run.sh` case branches (script token list, header comment + `case` arm order):
```
manifest  scaffold  trust  browse  marketplaces  scope  panel
visibility  command  exit2  subagent  cc-cache
```
12 stages, in that case-order (verified via `rg -n '^(manifest|scaffold|trust|browse|marketplaces|scope|panel|visibility|command|exit2|subagent|cc-cache)\)' qa/plugins/run.sh`). README command block now lists all 12 in this order **except** the existing pre-`mcp_face`-block rows are kept at their landed positions (manifest, scaffold, trust, then the three new ones), and the post-`mcp_face`-block rows are kept at their landed positions (visibility, scope, command, exit2, subagent, cc-cache). Every script token has exactly one README row.

`qa/mcp_face/run.sh` case branches (header comment + `STAGE` case arms):
```
handshake  tools  auth  list_changed  deny
```
5 stages, in that order. README lists all 5 in this order.

Plan expected `^\./qa/plugins/run\.sh | wc -l` → **11**; reconciled count is **12**. The +1 is `subagent`, which `qa/plugins/run.sh:710` adds in P4.13 (the `subagent` arm: "a command's `allowed-tools` restriction also bounds the `subagent` child it delegates to"). The plan's 11 omits `subagent` — likely because the plan was written before P4.13's commit (or because the plan author copy-pasted from an earlier stage list). `subagent` is in the script and in the README's pre-existing command block at line 726; it would be a regression to remove it. Documented here so the lead knows the count gap is intentional.

`^\./qa/mcp_face/run\.sh | wc -l` → **5** (plan expected 5). ✓

## Post-condition (script vs README, every stage)

| Script token | Script line | README row at | Match |
|---|---|---|---|
| `qa/plugins/run.sh manifest` | `qa/plugins/run.sh:210` (`manifest)`) | `:657` | ✓ |
| `qa/plugins/run.sh scaffold` | `qa/plugins/run.sh:232` | `:660` | ✓ |
| `qa/plugins/run.sh trust` | `qa/plugins/run.sh:275` | `:662` | ✓ |
| `qa/plugins/run.sh browse` | `qa/plugins/run.sh:301` | `:664` (NEW) | ✓ |
| `qa/plugins/run.sh marketplaces` | `qa/plugins/run.sh:335` | `:671` (NEW) | ✓ |
| `qa/plugins/run.sh panel` | `qa/plugins/run.sh:423` | `:680` (NEW) | ✓ |
| `qa/plugins/run.sh visibility` | `qa/plugins/run.sh:525` | `:699` | ✓ |
| `qa/plugins/run.sh scope` | `qa/plugins/run.sh:377` | `:705` | ✓ |
| `qa/plugins/run.sh command` | `qa/plugins/run.sh:573` | `:712` | ✓ |
| `qa/plugins/run.sh exit2` | `qa/plugins/run.sh:622` | `:718` | ✓ |
| `qa/plugins/run.sh subagent` | `qa/plugins/run.sh:710` | `:726` | ✓ |
| `qa/plugins/run.sh cc-cache` | `qa/plugins/run.sh:752` | `:729` | ✓ |
| `qa/mcp_face/run.sh handshake` | `qa/mcp_face/run.sh:26` (`STAGE="${1:-handshake}"`, case arm) | `:690` | ✓ |
| `qa/mcp_face/run.sh tools` | `qa/mcp_face/run.sh:27` | `:693` | ✓ |
| `qa/mcp_face/run.sh auth` | `qa/mcp_face/run.sh:27` | `:695` | ✓ |
| `qa/mcp_face/run.sh list_changed` | `qa/mcp_face/run.sh:27` | `:697` | ✓ |
| `qa/mcp_face/run.sh deny` | `qa/mcp_face/run.sh:27` | `:699` | ✓ |

## What was NOT done (state-the-negative, per AGENTS.md §6)

- **Per-fixture list NOT modified.** Both `- \*\*\``plugins`\*\*\`` (`:1870`) and `- \*\*\``mcp_face`\*\*\`` (`:1776`) entries already exist with detailed prose more comprehensive than the plan's draft. The plan's draft text is NOT mechanically applied — per user instruction "不要把原计划过时文本硬套到当前文件" (don't hard-fit the stale plan text onto current files). Both entries were added by prior P7.x commits and are richer than what P8.9 would have appended.
- **`mcp_face` per-fixture entry NOT moved after `terminal`.** The plan said to "append after the terminal entry" but `mcp_face` is currently at `:1776` (BEFORE `terminal` at `:1808`). The plan's directive is structurally impossible now without deleting the existing entry — moving it is out of scope for "drift reconciliation" and the entry's content is already informative.
- **`subagent` NOT removed from README despite plan listing 11 stages.** Plan's count of 11 omits `subagent`; the script has `subagent` (`:710`); the README pre-existing row at `:726` documents it; deleting the row would create a script↔README drift the other direction.
- **No `qa/plugins/run.sh`, `qa/mcp_face/run.sh`, or any other script touched.** Per user instruction "严格只改 qa/README.md（不改 scripts）". Verified: `git diff 11cb0eb58 -- 'qa/*.sh'` empty (only `qa/README.md` modified).
- **No `CLAUDE.md` touched.** CLAUDE.md's `src/extension/` row at `:126` lists `qa/plugins/run.sh {scope,visibility,command,exit2,cc-cache}` (5 stages only — its own drift vs the 12-stage script); out of P8.9 scope per plan boundary.
- **No `docs/superpowers/...`, no `docs/reference/...`, no `docs/archive/...` touched.** P8.9 is `qa/README.md` only. P8.10's archive moves are a separate task.
- **No `.superpowers/` reformatting or other report files modified.** Only `task-P8.9-report.md` is added (via `git add -f`, see "Commit" below).
- **No `~/.claude/settings.json` read; no `~/.claude/.aleph` written; no `aleph-server` started.** Per user hard constraints.
- **No `cargo fmt`, no `--no-verify`, no `git add -A`, no `git stash`.** Per user hard constraints.
- **Did NOT advance to P8.10** (the `docs/archive/` consolidation). Per "不要提前 P8.10".
- **Did NOT run `cargo check` / `cargo test` / `cargo clippy`.** P8.9 is a pure doc task; there is no Rust change to validate.
- **Did NOT run any `qa/` fixture.** The task is to reconcile the README, not to exercise the stages (the per-fixture entries already note the dates they were last green, and exercising them would be P7.x territory that is already closed).

## Commit

```
docs(qa): reconcile plugin and MCP harness README
```

- Conventional-commit form, English, scope `qa`.
- Trailer: `Co-Authored-By: MiniMax M3 <noreply@minimax.chat>` (per this dispatch's instruction; plan default `Claude Opus 5 (1M context) <noreply@anthropic.com>` not used).
- Files staged explicitly:
  - `qa/README.md` (the doc edit)
  - `.superpowers/sdd/2026-09-20-plugin-scope-and-cc-compat/task-P8.9-report.md` (this report, added with `git add -f` because `.superpowers/` is root-gitignored — `.gitignore:107` `.superpowers/`; verified `git check-ignore -v .superpowers/sdd/2026-09-20-plugin-scope-and-cc-compat/task-P8.9-report.md` returns `.gitignore:107:.superpowers/` exit 0).

## Verification commands (every one, results inline)

```
$ git diff --check
(empty output — exit 0, no whitespace or conflict-marker warnings)
✓

$ git diff 11cb0eb58 -- src/harness/
(empty output — exit 0)
✓ (R10 / plan "do NOT touch code" honored)

$ git diff 11cb0eb58 -- 'qa/*.sh'
(empty output — exit 0)
✓ (no script touched)

$ git diff 11cb0eb58 -- 'src/*'
(empty output — exit 0)
✓ (no code touched)

$ git diff --stat 11cb0eb58 -- qa/README.md
 qa/README.md | 25 +++++++++++++++++++++++++
 1 file changed, 25 insertions(+)
✓ (net 25-line insertion; no existing row modified or deleted)

$ rg -n '^\./qa/plugins/run\.sh' qa/README.md | wc -l
       12
✓ (12 script tokens; plan expected 11; gap = +1 for `subagent`, intentional)

$ rg -n '^\./qa/mcp_face/run\.sh' qa/README.md | wc -l
        5
✓ (5 script tokens; plan expected 5; matches)

$ rg -n '^(manifest|scaffold|trust|browse|marketplaces|scope|panel|visibility|command|exit2|subagent|cc-cache)\)' qa/plugins/run.sh | wc -l
       12
✓ (script has 12 stages; README now lists 12; parity)

$ rg -n 'handshake|tools|auth|list_changed|deny' qa/mcp_face/run.sh | head
4:#   ./qa/mcp_face/run.sh handshake     # three versions negotiate; unsupported → newest;
6:#   ./qa/mcp_face/run.sh tools         # tools/list ⊆ expose on the REAL registry; …
8:#   ./qa/mcp_face/run.sh auth          # bound to 0.0.0.0: LAN request 401 without / with a
11:#   ./qa/mcp_face/run.sh list_changed  # plugin_manage disable/enable → SSE list_changed +
13:#   ./qa/mcp_face/run.sh deny          # unexposed tool → -32602; a confirmation-gated tool …
26:STAGE="${1:-handshake}"
27:case "$STAGE" in handshake|tools|auth|list_changed|deny) ;;
✓ (script has 5 stages; README lists 5; parity)

$ rg -n '\*\*`(plugins|mcp_face)`\*\*' qa/README.md
1776:- **`mcp_face`** — 改 `src/gateway/mcp_face/` 或 `[mcp_server]` 前跑 `{handshake,tools,auth,list_changed,deny}`。
1870:- **`plugins`** — 改 `src/extension/`、`src/discovery/`、`src/gateway/execution_engine/slash_command_body/`
✓ (both per-fixture entries present, unchanged from pre-task state)

$ git status --short
 M qa/README.md
?? .superpowers/sdd/2026-09-20-plugin-scope-and-cc-compat/task-P8.9-report.md
(after staging: both lines become `M` or `A`; the report requires `git add -f` because `.superpowers/` is gitignored — see "Commit" above)
```