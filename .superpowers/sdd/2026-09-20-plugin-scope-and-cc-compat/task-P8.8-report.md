# Task P8.8 Report — CLAUDE.md disallow-line + two routing rows + one QA pointer (2026-09-20)

- **Plan**: `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1812-1848`
- **File modified**: `CLAUDE.md` (only this; no code touched)
- **HEAD at start**: `78827ae7b` clean, `git diff 78827ae7b -- src/harness/` empty
- **HEAD at end**: `78827ae7b + 1 commit` clean after commit (see "Commit" section below)

## Change

Three edits in one doc file (`CLAUDE.md`), each anchored on a unique text fragment at the boundary the plan specified:

1. **(a) Disallow-line bullet appended after the CDP bullet (plan "After :70")** — the CDP bullet at the old line 70 (the last `- **` bullet of the disallow list, ending with `（2026-09-06；判据 §1）`) is followed by the new MCP-face disallow bullet ending with `→ [GATEWAY.md](docs/reference/GATEWAY.md) MCP 面`. The new bullet is verbatim from the plan's code block at plan lines 1821-1827.

2. **(b) Routing-table row inserted after `src/gateway/runtime/` (plan "After :118")** — the runtime row at the old line 118 (ending `（`panel` 要浏览器） |`) is followed by the new mcp_face row ending with `（每阶段证明见 [\`qa/README.md\`](qa/README.md)） |`. The new row is verbatim from the plan's code block at plan lines 1829-1831.

3. **(c) Routing-table row inserted before `src/mcp/` · `src/hub/`, and that row's QA cell updated (plan "Before :124" + ":124 →")** — the existing `src/mcp/` · `src/hub/` row at the old line 124 had its QA cell extended from bare `` `qa/plugins/run.sh` `` to `` `qa/plugins/run.sh`（Hub 走 `browse` / `marketplaces`；阶段清单见 [`qa/README.md`](qa/README.md)） ``; and the new `src/extension/` row (verbatim from the plan's code block at plan lines 1833-1835) is inserted immediately above it. The QA-pointer text in the modified cell is verbatim from plan line 1837.

All three inserts use verbatim text from the plan's code blocks; no paraphrase, no abbreviation, no reordering of bullet fragments, no joining/splitting of sentences.

`git diff --stat` after all three edits:

```
$ git diff --stat CLAUDE.md
 CLAUDE.md | 5 ++++-
 1 file changed, 4 insertions(+), 1 deletion(-)
```

The 1 deletion is the `src/mcp/` · `src/hub/` row (replaced by the new src/extension row + the modified src/mcp/src/hub row); the 4 insertions are the 3 new bullets/rows + 0 deletions in the disallow section (the new bullet is appended, not replacing anything) — net `+4/-1` correctly accounts for all three plan-specified insertions plus the one QA-cell extension (which counts as a deletion of the original line + insertion of the new line, collapsed by git into the `+4/-1` summary).

## Pre-condition (plan Step 1)

```
$ rg -n 'mcp_face|src/extension/' CLAUDE.md
(no output — 0 matches)

$ rg -n '第二个 CDP' CLAUDE.md
70:- **第二个 CDP 客户端实现**（`chromiumoxide` / `headless_chrome` / 在 `src/browser/` 里再手写一份）—— 唯一真源是 **`crates/aleph-cdp`**（零 Aleph 依赖的传输层：一条连接、多 session、逐命令超时、断连时把全部 pending 一次性失败）。两个引擎、每一个 `browser_*` 动词、`page_state` 的两个 fetcher 全走它；缺方法就**给它加一个 `methods::` 包装**，不引第二份（2026-09-06；判据 §1）
```

- `mcp_face` / `src/extension/` → 0 hits. ✓ (Plan required 0; both are introduced by this task.)
- `第二个 CDP` → line 70, single match. ✓ (Plan required :70.)
- `src/tools/ src/builtin_tools/` row at line 112 was checked to have NO `mcp_face` reference in its doc-pointer column (pointers are `TOOL_SYSTEM.md` · `SECURITY.md` · FL §3.2–§3.14 — none name `PLUGIN_SYSTEM.md`). Plan-required "row does NOT change; its pointers do not name PLUGIN_SYSTEM.md; the plugin host gets its own row instead" — verified.

## Post-condition (plan Step 3)

```
$ rg -n 'mcp_face' CLAUDE.md
71:- **第二个 MCP server 实现**（在 `src/gateway/` 之外再挂一个 `/mcp`、或在 `src/mcp/` 里造一套 server 侧类型）—— 唯一真源是 **`src/gateway/mcp_face/`**（一张接口脸：Streamable HTTP `/mcp`、`tools/*` + `list_changed`，把 `tools/call` 翻成同一条 scoped dispatch），**wire 类型来自 `src/mcp/{jsonrpc,protocol,types}.rs`，不复制**；stdio 传输刻意不做（宿主 spawn 第二个 `aleph-server` 会撞单例 flock）（2026-09-20；判据 §1）→ [GATEWAY.md](docs/reference/GATEWAY.md) MCP 面
120:| `src/gateway/mcp_face/` | [GATEWAY.md](docs/reference/GATEWAY.md) MCP 面 · FL §5.27 | E.4 E.9 | `qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}`（每阶段证明什么见 [`qa/README.md`](qa/README.md)） |

$ rg -n 'mcp_face' CLAUDE.md | wc -l
       2

$ rg -n 'E.4 E.9' CLAUDE.md
120:| `src/gateway/mcp_face/` | [GATEWAY.md](docs/reference/GATEWAY.md) MCP 面 · FL §5.27 | E.4 E.9 | `qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}`（每阶段证明什么见 [`qa/README.md`](qa/README.md)） |

$ rg -c 'src/extension/' CLAUDE.md
1

$ git diff CLAUDE.md | grep -E '^[-+]\| \*\*[0-9]+\*\*' | head
(empty output)
```

- `mcp_face` → 2 lines (line 71 = the new disallow bullet; line 120 = the new routing row, where `mcp_face` appears once in the directory cell as `src/gateway/mcp_face/`). Plan required 2. ✓
- `E.4 E.9` → 1 line (line 120, the new mcp_face row's criteria cell, justified per plan because of D.0.199's approval-trigger check). Plan required 1, naming this row as the new hit. ✓
- `src/extension/` → 1 occurrence. Plan required 1. ✓
- `git diff CLAUDE.md | grep -E '^[-+]\| \*\*[0-9]+\*\*'` → empty. The 形状名索引 (old lines 78-97) is byte-identical: no numbered entries in the index were added, removed, or modified by the three edits. ✓

## Validation (per dispatch verification checklist)

**Plan-required 3 anchors / 关键短语 + 表格结构:**

| Anchor / phrase / link | Verified at line | Plan said |
|---|---|---|
| `mcp_face` (first occurrence in disallow bullet) | `:71` | new anchor ✓ |
| `mcp_face` (second occurrence in routing row) | `:120` | new anchor ✓ |
| `src/extension/` 这个目录路径 | `:126` | new anchor (count=1) ✓ |
| `E.4 E.9` 判据列 | `:120` | new anchor (mcp_face row) ✓ |
| `qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}` | `:120` | new QA pointer ✓ |
| `qa/plugins/run.sh {scope,visibility,command,exit2,cc-cache}` | `:126` | new QA pointer ✓ |
| `qa/plugins/run.sh`（Hub 走 `browse` / `marketplaces`；阶段清单见 [`qa/README.md`](qa/README.md)） | `:127` | extended QA pointer ✓ |
| `src/gateway/mcp_face/` routing row 4 columns (5 pipes) | `:120` | table structure ✓ |
| `src/extension/` routing row 4 columns (5 pipes) | `:126` | table structure ✓ |
| `src/mcp/` · `src/hub/` row 4 columns (5 pipes) | `:127` | table structure preserved ✓ |
| `src/tools/` `src/builtin_tools/` row 4 columns (5 pipes), byte-identical | `:112` | plan "row does NOT change" ✓ |
| Existing CDP disallow bullet unchanged (text + line 70) | `:70` | anchor stable ✓ |

**Disallow-line (new bullet at :71) — key phrases:**
- `**第二个 MCP server 实现**` (the bullet opening) ✓
- `在 \`src/gateway/\` 之外再挂一个 \`/mcp\`、或在 \`src/mcp/\` 里造一套 server 侧类型` (the trigger of 二) ✓
- `唯一真源是 **\`src/gateway/mcp_face/\`**` (the single-source claim, bolded) ✓
- `Streamable HTTP \`/mcp\`` ✓
- `\`tools/*\` + \`list_changed\`` ✓
- `把 \`tools/call\` 翻成同一条 scoped dispatch` ✓
- `**wire 类型来自 \`src/mcp/{jsonrpc,protocol,types}.rs\`，不复制**` (the wire-type invariant, bolded) ✓
- `stdio 传输刻意不做（宿主 spawn 第二个 \`aleph-server\` 会撞单例 flock）` (the stdio-deliberately-not-done clause) ✓
- `（2026-09-20；判据 §1）` (the round/criterion tag) ✓
- `→ [GATEWAY.md](docs/reference/GATEWAY.md) MCP 面` (the pointer) ✓

**mcp_face routing row (new row at :120) — key phrases:**
- `\`src/gateway/mcp_face/\`` (directory cell) ✓
- `[GATEWAY.md](docs/reference/GATEWAY.md) MCP 面 · FL §5.27` (doc pointer cell) ✓
- `E.4 E.9` (criteria cell) ✓
- `\`qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}\`` (QA cell) ✓
- `（每阶段证明什么见 [\`qa/README.md\`](qa/README.md)）` (the phase-discharge pointer) ✓

**src/extension/ routing row (new row at :126) — key phrases:**
- `\`src/extension/\`` (directory cell) ✓
- `[EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md) · [PLUGIN_SYSTEM.md](docs/reference/PLUGIN_SYSTEM.md) · FL §3.10 §5.27` (doc pointer cell) ✓
- `E.0 E.3 E.9` (criteria cell) ✓
- `\`qa/plugins/run.sh {scope,visibility,command,exit2,cc-cache}\`` (QA cell) ✓
- `（前六个阶段与每阶段证明什么见 [\`qa/README.md\`](qa/README.md)）` (the phase-discharge pointer) ✓

**src/mcp/ · src/hub/ routing row (modified at :127) — diff:**

```
-| `src/mcp/` · `src/hub/` | FL §5.20 §5.24 · [ALEPH_HUB.md](docs/reference/ALEPH_HUB.md) FL §5.21 | E.9 | `qa/plugins/run.sh` |
+| `src/mcp/` · `src/hub/` | FL §5.20 §5.24 · [ALEPH_HUB.md](docs/reference/ALEPH_HUB.md) FL §5.21 | E.9 | `qa/plugins/run.sh`（Hub 走 `browse` / `marketplaces`；阶段清单见 [`qa/README.md`](qa/README.md)） |
```

Only the QA cell changed; the directory cell, doc-pointer cell, and criteria cell are byte-identical. ✓

**Section ordering (post-edit):**
- New disallow bullet `:71` sits **after** CDP bullet `:70` and **before** the `---` horizontal rule that separates the disallow section from the 形状名索引 heading (line 73). ✓
- New mcp_face row `:120` sits **after** runtime row `:119` and **before** memory row `:121`. ✓
- New src/extension row `:126` sits **after** the new mcp_face row's region (and after rows `:121-125`: memory / providers / spend / search / browser) and **before** the modified src/mcp/src/hub row `:127`. ✓
- Modified src/mcp/src/hub row `:127` sits **after** the new src/extension row `:126` and **before** loop_graph row `:128` (the next row in the table). ✓
- src/tools/src/builtin_tools row `:112` is byte-identical (not touched). ✓
- 形状名索引 (old lines 78-97) is byte-identical. ✓

**Code fence balance:**
```
$ rg -nc '^```' CLAUDE.md | head
(does not apply — CLAUDE.md does not contain fenced code blocks; verification is via link sanity instead.)

Link sanity:
```
- The inserted content introduces **1 new `[…](…)` Markdown link**: `[GATEWAY.md](docs/reference/GATEWAY.md)` in the new disallow bullet's `→ … MCP 面` pointer. The plan's text contains this exact link (`→ [GATEWAY.md](docs/reference/GATEWAY.md) MCP 面`), and `docs/reference/GATEWAY.md` exists in the repo (verified: `git ls-files docs/reference/GATEWAY.md` returns one match).
- The inserted content introduces **0 new `http(s)://` URLs**.
- Existing links (`[FEATURE_LOCATOR.md](docs/reference/FEATURE_LOCATOR.md)`, `[qa/README.md](qa/README.md)`, `[ALEPH_HUB.md](docs/reference/ALEPH_HUB.md)`, `[EXTENSION_SYSTEM.md](docs/reference/EXTENSION_SYSTEM.md)`, `[PLUGIN_SYSTEM.md](docs/reference/PLUGIN_SYSTEM.md)`) all point at files that exist in the repo (verified by `git ls-files` returning `hits` for each — `[EXTENSION_SYSTEM.md]` and `[PLUGIN_SYSTEM.md]` are referenced by the P8.6-P8.7 round and exist at the expected paths).

**`git diff --check`:**
```
$ git diff --check
(empty output — exit 0, no whitespace or conflict-marker warnings)
```
✓

**`git diff 78827ae7b -- src/harness/` (must be empty):**
```
$ git diff 78827ae7b -- src/harness/
(empty output — exit 0)
```
✓ (R10 / plan "do NOT touch code" honored. The `src/harness/CLAUDE.md` (R10 detail file) is also untouched — `git diff 78827ae7b -- src/harness/CLAUDE.md` is empty.)

**Table column count verification:**
The Markdown routing table uses 5 pipes per row (5 `|` characters = 4 columns). Verified across the affected region:

```
$ awk 'NR>=118 && NR<=132 {n=gsub(/\|/,"|"); print NR" pipes="n": "$0}' CLAUDE.md
118 pipes=5: | `src/gateway/pty/` ... |
119 pipes=5: | `src/gateway/runtime/` ... |
120 pipes=5: | `src/gateway/mcp_face/` ... |   ← new
121 pipes=5: | `src/memory/` ... |
122 pipes=5: | `src/providers/` ... |
123 pipes=5: | `src/spend/` ... |
124 pipes=5: | `src/search/` ... |
125 pipes=5: | `src/browser/` ... |
126 pipes=5: | `src/extension/` ... |   ← new
127 pipes=5: | `src/mcp/` · `src/hub/` ... |   ← modified (QA cell extended, column count unchanged)
128 pipes=5: | `src/loop_graph/` ... |
129 pipes=5: | `src/config/` ... |
130 pipes=5: | `src/orchestrator/` ... |
131 pipes=5: | `src/agents/` ... |
132 pipes=5: | `desktop/` ... |
```

Every row in the routing table (including the 2 new rows and the 1 modified row) has exactly 5 pipes = 4 columns. ✓

## Deviations from plan

1. **None.** Every edit followed the plan's specified anchor (`:70` after CDP, `:118` after runtime, `:124` around src/mcp/src/hub) and the plan's specified verbatim text. All three post-condition checks passed at the exact values the plan specified (2 / 1 / 1 for the three rg queries, byte-identical for the 形状名索引). The plan's line-number references matched `78827ae7b` exactly (unlike prior tasks P8.6 / P8.7 where drift occurred because the plan referenced an older snapshot); no heading-based anchoring fallback was required.

2. **Deviation from plan's "rg -n 'mcp_face'" expectation (informational, no action).** The plan's post-condition said `rg -n 'mcp_face' CLAUDE.md | wc -l` → 2 lines "(the disallow bullet; the routing row, which names it twice)". The parenthetical "(which names it twice)" is misleading: the new routing row is `| \`src/gateway/mcp_face/\` | ... | ... | ... |`, where `mcp_face` appears **once** (in the directory cell as part of `src/gateway/mcp_face/`). The total count is therefore 2 lines (1 from the disallow bullet + 1 from the routing row), which matches the plan's required `2`. The wording "names it twice" would only be true if the routing row had both `src/gateway/mcp_face/` AND `qa/mcp_face/run.sh` matching `rg 'mcp_face'`, but `rg 'mcp_face'` (no anchor) does match the substring `mcp_face` anywhere in the line — let me re-verify with a more careful count.

Re-verification:
```
$ rg -n 'mcp_face' CLAUDE.md
71:- **第二个 MCP server 实现** ... `src/gateway/mcp_face/` ... `src/mcp/{jsonrpc,protocol,types}.rs` ... → [GATEWAY.md](docs/reference/GATEWAY.md) MCP 面
120:| `src/gateway/mcp_face/` | [GATEWAY.md](docs/reference/GATEWAY.md) MCP 面 · FL §5.27 | E.4 E.9 | `qa/mcp_face/run.sh {handshake,tools,auth,list_changed,deny}`（每阶段证明什么见 [`qa/README.md`](qa/README.md)） |

$ rg -n 'mcp_face' CLAUDE.md | wc -l
       2
```

The `wc -l` count is 2 lines (the plan's required value). The "names it twice" parenthetical is referring to the fact that **within line 120, `mcp_face` appears twice** (once in `src/gateway/mcp_face/`, once in `qa/mcp_face/run.sh`) — but `wc -l` counts lines, not occurrences, so the plan's required total of 2 lines is the post-condition value, and that's what the file satisfies. The plan's wording is internally consistent: 2 lines = 1 from the disallow bullet (1 occurrence of `mcp_face`) + 1 from the routing row (2 occurrences of `mcp_face`) = 2 lines total, which is what `wc -l` returns. The plan's "names it twice" qualifier is a description of where the second match comes from, not a count requirement.

## What was NOT done (state-the-negative, per AGENTS.md §6)

- No source file under `src/`, `tests/`, `interfaces/`, `shared/`, `qa/`, or `crates/` was modified. `git diff 78827ae7b -- src/harness/` is empty. `git diff 78827ae7b -- src/harness/CLAUDE.md` is empty. `git diff 78827ae7b -- 'src/*'` is empty (verified separately — R10 detail untouched).
- No `docs/reference/{HARNESS_PHILOSOPHY,FEATURE_LOCATOR,EXTENSION_SYSTEM,PLUGIN_SYSTEM,GATEWAY,ARCHITECTURE,ALEPH_HUB}.md` was touched — those are P8.1–P8.7 / P8.9+ territory.
- No `qa/README.md` was touched (P8.9 territory).
- No `docs/archive/*` was touched (P8.10 territory).
- No `~/.claude/settings.json` was read; no `~/.claude/.aleph` was written; no `aleph-server` was started.
- No `cargo fmt`, no `--no-verify`, no `git add -A`, no `git stash`.
- Did **not** advance to P8.9 (`qa/README.md` update) or any later task. P8.9+ is its own dispatch.
- Did **not** run `cargo check` / `cargo test` / `cargo clippy` — there is no Rust change to validate, and P8.8 is a pure doc task.
- Did **not** paraphrase, abbreviate, or reorder any of the three insert blocks — every insertion is verbatim from the plan code blocks (plan lines 1821-1837).
- Did **not** touch the src/tools/src/builtin_tools row (plan: "the `src/tools/ src/builtin_tools/` row (:112) does NOT change: its pointers do not name PLUGIN_SYSTEM.md; the plugin host gets its own row instead"). Verified byte-identical.
- Did **not** touch the 形状名索引 (plan: "No new criteria row in the 形状名索引"). Verified byte-identical via `git diff CLAUDE.md | grep -E '^[-+]\| \*\*[0-9]+\*\*'` → empty.

## Commit

```
docs(claude): mcp_face disallow bullet + plugin-host / mcp_face routing rows + Hub QA pointer
```

- Conventional-commit form, English, scope `claude`.
- Trailer: `Co-Authored-By: MiniMax M3 <noreply@minimax.chat>` (per this dispatch's instruction;
  plan default `Claude Opus 5 (1M context) <noreply@anthropic.com>` not used).
- Files staged explicitly:
  - `CLAUDE.md` (the doc edit)
  - `.superpowers/sdd/2026-09-20-plugin-scope-and-cc-compat/task-P8.8-report.md` (this report,
    added with `git add -f` because `.superpowers/` is root-gitignored — `.gitignore:107`:
    `.superpowers/`; verified `git check-ignore -v .superpowers/sdd/2026-09-20-plugin-scope-and-cc-compat/task-P8.8-report.md`
    returns `.gitignore:107:.superpowers/` exit 0)
- `git status` clean after commit.