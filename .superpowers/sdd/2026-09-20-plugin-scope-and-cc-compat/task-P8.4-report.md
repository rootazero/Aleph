# Task P8.4 Report — EXTENSION_SYSTEM.md: "Effects and EffectScope" + "ScopeKey and visibility" sections, plus RPC table fix

Plan: `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md` §"Phase P8 — documentation" → "Task P8.4"
Worktree HEAD (pre): `6b690df62` (`docs(plugin-system): align runtime and scope contract`).
Scope: docs-only; one file modified, no `src/*.rs` touched.

## Files

- Modify: `docs/reference/EXTENSION_SYSTEM.md`
  - **Insert (Step 2 of plan)**: two sections immediately after `## Node plugins run as MCP stdio servers` (the section ending on the `---` divider at pre-edit line 168) and before `## Plugin Discovery` (pre-edit line 169):
    - `## Effects and `EffectScope` (temporal composability)` (verbatim text from plan, fenced with the 4-backtick outer fence in the plan; inserted as plain Markdown — code fence uses 3 backticks, `rust` lang)
    - `---` separator
    - `## `ScopeKey` and visibility (spatial composability)` (verbatim text from plan, same fence handling)
    - `---` separator
  - **Fix RPC table** (the table at plan `:403-413`, currently at file lines 423–434):
    - delete `| `plugins.reload` | Reload plugin |`
    - insert `| `plugin.reload` | Reload one plugin (unmount + mount) |` (plan exact text)
    - insert `| `plugins.callTool` | Call a tool on a loaded runtime plugin (CLI) |` (plan exact text)

No other file touched.

## Anchoring rationale (plan line numbers had drifted)

Plan referenced `:403-413` for the RPC table and `:412` for the `plugins.reload` row. At `6b690df62`:
- `## Plugin RPC Methods` is at line **423**, table body at lines **425-434**
- `| `plugins.reload` | Reload plugin |` is at line **432**

The plan's anchoring-by-heading intent was preserved (the table is uniquely identified by its heading, and the `plugins.reload` row is uniquely identified within it). Insertion point for the new sections was anchored by the trailing `---` of the Node plugins section (the prose immediately above the insertion already matches verbatim, so the `---` line is unique in the file at pre-edit position).

## Verification (paste-form, run on this worktree at HEAD)

- **Pre-condition (plan Step 1)**
  - `rg -n 'EffectScope|visible_to' docs/reference/EXTENSION_SYSTEM.md` → 1 hit (`152:registers its tools, and `unmount` stops it (EffectScope step `"mcp_server"`)` — pre-existing single mention in the Node plugins section, no `visible_to` anywhere). The plan expected 0; the deviation is documented under "Deviations" below.
  - `rg -n 'plugins\.reload' docs/reference/EXTENSION_SYSTEM.md` → `:432` (one hit, the table row that this task replaces).
- **Post-condition (plan Step 3)**
  - `rg -n 'EffectScope|visible_to|has an inverse' docs/reference/EXTENSION_SYSTEM.md | wc -l` → **8** (plan required ≥ 6). Hits: line 152 (pre-existing), 169 (section heading), 185 (struct), 186 (impl block), 199 (the one rule), 202 (prose), 227 (`visible_to` signature), 235 (`visible_to` at request-build time).
  - `rg -n 'plugins\.reload' docs/reference/EXTENSION_SYSTEM.md` → **0** (the row was replaced; `plugin.reload` and `plugins.callTool` are the new rows).
- **Table content (post-edit)**
  - `rg -n 'plugin\.reload' docs/reference/EXTENSION_SYSTEM.md` → `:432` (new row, `Reload one plugin (unmount + mount)`).
  - `rg -n 'plugins\.callTool' docs/reference/EXTENSION_SYSTEM.md` → `:433` (new row, `Call a tool on a loaded runtime plugin (CLI)`).
  - `rg -n 'plugins\.reload' docs/reference/EXTENSION_SYSTEM.md` → 0 (the retired verb is gone from the table).
- **Code fence balance**
  - `rg -nc '^```' docs/reference/EXTENSION_SYSTEM.md` → **50** (even; 25 fenced blocks).
- **Section ordering (post-edit heading structure)**
  ```
  7:   ## Overview
  18:  ## Architecture
  50:  ## Plugin Structure
  90:  ## WASM Runtime
  143: ## Node plugins run as MCP stdio servers
  169: ## Effects and `EffectScope` (temporal composability)        ← NEW
  218: ## `ScopeKey` and visibility (spatial composability)         ← NEW
  244: ## Plugin Discovery
  279: ## Plugin Registry
  ...
  423: ## Plugin RPC Methods                                        ← TABLE FIXED
  ```
  The two new sections sit exactly where the plan required: between `## Node plugins run as MCP stdio servers` and `## Plugin Discovery`. `## Plugin RPC Methods` remains single, with the corrected rows.
- **Title text** matches plan verbatim — backticks around `EffectScope` / `ScopeKey` are preserved, parentheses `(temporal composability)` / `(spatial composability)` preserved.
- **Code fence lang** — both inserted rust blocks declare ```` ```rust ```` fences; lines 170–184 and 221–227.
- **Link sanity** — `HARNESS_PHILOSOPHY.md` and `scan-dsh-cordis.md` references in the inserted prose are doc-relative paths (no broken anchors); the `executor.rs:918` reference matches the plan exactly.
- **`git diff --check`** → no whitespace or conflict-marker warnings on the working-tree diff.
- **`git diff 6b690df62 -- src/harness/`** → 0 lines (R10 / plan Step "do NOT touch code" honored; verified before AND after the edits).
- **`git status --short`** before staging → ` M docs/reference/EXTENSION_SYSTEM.md` plus the new report (untracked).

## Boundaries / negatives

- No mutation step applies (P8.4 is a pure doc insertion + table fix — there is no Rust guard to go red; the post-condition greps ARE the tests, per the plan's Step 3 post-condition that pins the counts directly).
- Did NOT touch `docs/reference/PLUGIN_SYSTEM.md`, `ARCHITECTURE.md`, `ALEPH_HUB.md`, `SKILL_MODEL_TAXONOMY.md`, `FEATURE_LOCATOR.md`, `HARNESS_PHILOSOPHY.md`, `CLAUDE.md`, `qa/README.md`, or `docs/archive/` slots — those are P8.1–P8.3 and P8.5–P8.10 territory.
- Did NOT touch any `src/*.rs`, `interfaces/`, `shared/`, `qa/`, `crates/`, or `Cargo.toml`. The plan's Step 1 lists `src/extension/effects/{mod.rs, scope.rs, disposer.rs}` and `src/extension/visibility.rs` as locations the prose cites — those code paths exist in the tree from earlier phases, but this task is doc-only and references them by path without modifying them.
- Did NOT run `cargo check` / `cargo test` / `cargo fmt` — no Rust changes in this task.
- Did NOT touch P8.5+ (`GATEWAY.md` MCP section, `HARNESS_PHILOSOPHY.md` 第五课补注, `FEATURE_LOCATOR.md` round entry / §5.27 / D.0.196–199 / E triggers, `CLAUDE.md` disallow-list + routing rows) — that is later dispatch territory.
- Did NOT read `~/.claude/settings.json`; did NOT write `~/.claude/.aleph`; did NOT start `aleph-server`; did NOT use `--no-verify`, `git add -A`, `cargo fmt`, or bare `git stash`.

## Deviations from plan

1. **Pre-edit `EffectScope` count was 1, not 0** (line 152 inside the `## Node plugins run as MCP stdio servers` section: ``registers its tools, and `unmount` stops it (EffectScope step `"mcp_server"`).``). The plan said "rg -n 'EffectScope|visible_to' docs/reference/EXTENSION_SYSTEM.md → 0" as a pre-condition. The deviation is informational — the plan's post-condition is `>= 6`, and we hit **8** — but the line 152 mention was already there from the P5.7 prose edit, and the plan insertion text intentionally does not edit that line (it just adds new sections around it). No code change needed; the post-condition still passes by a comfortable margin.
2. **Plan line-number anchors drifted**: plan said "before `## Plugin Discovery`" and `:403-413` for the table. At `6b690df62`, `## Plugin Discovery` is at line 169 (insertion was anchored on the `---` divider at line 168, which is unique in the file) and `## Plugin RPC Methods` is at line 423 with the `plugins.reload` row at line 432 (table fix was anchored on the unique row text `| `plugins.reload` | Reload plugin |`, which only appears once in the file). The task description explicitly said "按标题/锚文本执行" so this is the documented working method, not a deviation in spirit.

## Commit

- `docs(extension-system): Effects/EffectScope and ScopeKey/visibility sections; fix the RPC table` (conventional, English, scope `extension-system`).
- Trailer: `Co-Authored-By: MiniMax M3 <noreply@minimax.chat>` (per this dispatch's instruction; plan default `Opus 5.5` not used).
- Files staged: `docs/reference/EXTENSION_SYSTEM.md` (explicit) + `.superpowers/sdd/2026-09-20-plugin-scope-and-cc-compat/task-P8.4-report.md` (forced add — `.superpowers/` is root-gitignored).