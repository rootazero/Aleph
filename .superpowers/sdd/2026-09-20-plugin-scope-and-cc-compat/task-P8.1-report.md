# Task P8.1 Report — ARCHITECTURE.md: drop the phantom `src/clawhub/` row

Plan: `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md` §"Phase P8 — documentation" → "Task P8.1"
Worktree HEAD (pre): `44b05e31e` (`qa/mcp_face: read 401 response headers from Mcp.last_headers so stage_auth runs end-to-end`).
Scope: docs-only; one file modified, no `src/*.rs` touched.

## Files

- Modify: `docs/reference/ARCHITECTURE.md`
  - delete `| **clawhub** | \`src/clawhub/\` | ClawHub integration |` (was line 261)
  - add (per plan Step 2 fallback, since `**hub**` was absent) `| **hub** | \`src/hub/\` | Aleph Hub — extension catalog + install pipeline ([ALEPH_HUB.md](./ALEPH_HUB.md)) |` in alphabetical position immediately after the `**group_chat**` row (now at 268 → hub at 269)

No other file touched.

## Verification (paste-form, run on this worktree at HEAD)

- **Step 1 pre-condition**
  - `ls src/clawhub` → `ls: src/clawhub: No such file or directory`
  - `rg -n 'src/clawhub' docs/reference/ARCHITECTURE.md` → exactly `:261` (single hit, the phantom row)
  - `rg -n '\*\*hub\*\*' docs/reference/ARCHITECTURE.md` → 0 (the fallback branch in plan Step 2 applies)
- **Step 3 post-condition**
  - `rg -n -i 'clawhub' docs/reference/ARCHITECTURE.md` → 0 (exit=1, no matches)
  - `rg -n '\*\*hub\*\*' docs/reference/ARCHITECTURE.md` → `:269` (one match, in alphabetical position between `group_chat` and `intent`)
- **Alphabetical ordering preserved** in the row block around the edit (`cluster` → `components` → `compressor` → `core` → `discovery` → `event` → `generation` → `group_chat` → `hub` → `intent`).
- **Link sanity** — `[ALEPH_HUB.md](./ALEPH_HUB.md)` resolves to the existing file (`docs/reference/ALEPH_HUB.md`, 17064 B, mtime preserved).
- **`git diff --check`** → no whitespace or conflict-marker warnings on the working-tree diff.
- **`git diff 44b05e31e -- src/harness/`** → 0 lines (R10 honored; verified before AND after the edit).
- **`git status --short`** → only ` M docs/reference/ARCHITECTURE.md` until the report is staged with `-f`.

## Boundaries / negatives

- No mutation step applies (P8.1 is a pure doc deletion — there is no guard to go red; the post-condition grep IS the test, per the phase header "the grep IS the test and it is stated with its expected output").
- Did NOT touch `docs/reference/ALEPH_HUB.md`, `PLUGIN_SYSTEM.md`, `EXTENSION_SYSTEM.md`, `SKILL_MODEL_TAXONOMY.md`, `FEATURE_LOCATOR.md`, `HARNESS_PHILOSOPHY.md`, `CLAUDE.md`, `qa/README.md`, or the `docs/archive/` slot — those are P8.2–P8.10 territory.
- Did NOT touch any `src/*.rs`, `interfaces/`, `shared/`, `qa/`, or `Cargo.toml`.
- Did NOT run `cargo check` / `cargo test` / `cargo fmt` — no Rust changes in this task.
- Did NOT read `~/.claude/settings.json`; did NOT write `~/.claude/.aleph`; did NOT start `aleph-server`; did NOT use `--no-verify`, `git add -A`, `cargo fmt`, or bare `git stash`.
- Pre-existing red mentioned in constraints.md is in `src/capability/census.rs` and is unrelated to this task (no Rust touched).

## Commit

- `docs(architecture): drop the src/clawhub/ row — the directory never existed` (conventional, English, scope `architecture`).
- Trailer: `Co-Authored-By: MiniMax M3 <noreply@minimax.chat>` (per this dispatch's instruction; plan default `Opus 5.5` not used).
- Files staged: `docs/reference/ARCHITECTURE.md` (explicit) + `.superpowers/sdd/2026-09-20-plugin-scope-and-cc-compat/task-P8.1-report.md` (forced add — `.superpowers/` is root-gitignored).
