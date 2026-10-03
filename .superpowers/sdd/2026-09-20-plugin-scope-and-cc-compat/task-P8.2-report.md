# Task P8.2 Report — ALEPH_HUB.md: keep every ruling, drop the OpenClaw comparison framing

Plan: `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md` §"Phase P8 — documentation" → "Task P8.2" (`P5P8-cut-and-docs.md:1248-1327`, coverage map row at `:2045`).
Worktree HEAD (pre): `d5dd3ac4b` (`docs(architecture): drop the src/clawhub/ row — the directory never existed`).
Scope: docs-only; one file modified, no `src/*.rs` touched.

## Files

- Modify: `docs/reference/ALEPH_HUB.md`
  - `:3-5` — blockquote: drop `openclaw clawhub 逐项对照表` → point at `§7 的设计裁定清单`.
  - `:54` — JSON example: `"via": "clawhub"` → `"via": "github:acme"` (matched source-label example used elsewhere in the table).
  - `:230-257` — entire §7 (heading through the `刻意不移植` paragraph) replaced with the 设计裁定 table that drops the comparison framing. Subject column rewritten from `维度 | openclaw | Aleph 现状` to `能力 | Aleph 的裁定 | 理由`. All 13 rulings preserved (✅/⚠️/❌/N/A and rationale column = plan's verbatim text). Heading retitled `## 7. 设计裁定：有意不做的能力（与它们的理由）`, with a one-line note that this section used to be a comparison table (history pointer only).
  - `:267` (DEVIATION, see below) — §8 item 3: `真要做的对位是 openclaw 的 sqlite 租约` → `真要做的对位是安装期 sqlite 租约 + 心跳` (drop `openclaw 的` to match the new framing; same content, no comparison).

No other file touched.

## Plan factual error / deviation

The plan's Step 1 says "→ 14 hits (evidence §5.1 count for this file; paste)" but the actual file at `d5dd3ac4b` has 14 hits TOTAL of which only 13 fall inside the target ranges (`:3-5, :54, :230-257`); the 14th is at `:267` (now 259 after the §7 rewrite pre-edit), inside §8 item 3, **outside** the target ranges. Step 3 explicitly covers this contingency: "if any hit remains outside `:3-5, :54, :230-257`, quote it and relabel in the same commit — the 14 pre-count says there are none, but the grep decides."

Deviation applied (in the same commit): rewrote `:267` from `…真要做的对位是 openclaw 的 sqlite 租约。` to `…真要做的对位是安装期 sqlite 租约 + 心跳。`. This brings the post-condition grep to 0 and is content-preserving (the `sqlite 租约 + 心跳` design lives on as a self-contained sentence; §7's new "并发租约" row already names the same shape, so the §8 reference remains coherent without the attribution).

The Step 1 pre-count of 14 was misleading (it reads as "14 inside the target ranges", but only 13 are). Lesson for the lead: when the plan asserts a pre-count, run the grep with `grep -n` (not just `grep -c`) and partition by anchor range before relying on the post-condition.

## Verification (paste-form, run on this worktree at HEAD)

- **Step 1 pre-condition** — `grep -n -i 'openclaw\|clawhub' docs/reference/ALEPH_HUB.md` → 14 hits at lines `4, 54, 230, 232, 233, 236, 238, 239, 240, 244, 249, 250, 252, 267` (13 inside target ranges, 1 outside in §8 — see deviation above).
- **Step 2 replacements applied** — three plan ranges (`:3-5, :54, :230-257`) replaced verbatim with plan-specified text; fourth edit at `:267` applied as deviation per Step 3 contingency.
- **Step 3 post-condition** — `grep -n -i 'openclaw\|clawhub' docs/reference/ALEPH_HUB.md` → exit=1, no matches (the post-condition target of 0 hits is met AFTER the deviation relabel).
- **Diff sanity**
  - `git diff --stat docs/reference/ALEPH_HUB.md` → `1 file changed, 22 insertions(+), 27 deletions(-)`.
  - `git diff --check` → exit=0, no whitespace or conflict-marker warnings.
  - `git diff d5dd3ac4b -- src/harness/` → empty (R10 honored; the doc-only constraint is respected — no harness/ source touched).
- **Link sanity** — the `[FEATURE_LOCATOR.md §5.21](FEATURE_LOCATOR.md)` link in the §0 blockquote resolves to the existing file (`docs/reference/FEATURE_LOCATOR.md`). The hub↔ALEPH_HUB.md cross-reference set up by P8.1 (`ARCHITECTURE.md:269` → `[ALEPH_HUB.md](./ALEPH_HUB.md)`) is intact.
- **Section ordering** — `grep -n '^## ' docs/reference/ALEPH_HUB.md` → `1.` at 9, `2.` at 26, `3.` at 94, `4.` at 109, `5.` at 157, `6.` at 210, `7.` at 230, `8.` at 254, `See Also` at 269. The new §7's "见 §8" rows reference the §8 anchor at 254 — still valid after the rewrite.
- **Table row count** — new §7 has exactly 14 data rows (the original had 14 rows as well; plan preserved all rulings and renames the subject column only).

## Boundaries / negatives

- Did NOT touch `docs/reference/PLUGIN_SYSTEM.md`, `EXTENSION_SYSTEM.md`, `SKILL_MODEL_TAXONOMY.md`, `FEATURE_LOCATOR.md`, `HARNESS_PHILOSOPHY.md`, `CLAUDE.md`, `qa/README.md`, `ARCHITECTURE.md`, or the `docs/archive/` slot — those are P8.1 / P8.3+ territory.
- Did NOT touch any `src/*.rs`, `interfaces/`, `shared/`, `qa/`, `Cargo.toml`, or `justfile`.
- Did NOT run `cargo check` / `cargo test` / `cargo fmt` — no Rust changes in this task.
- Did NOT read `~/.claude/settings.json`; did NOT write `~/.claude/.aleph`; did NOT start `aleph-server`; did NOT use `--no-verify`, `git add -A`, `cargo fmt`, or bare `git stash`.
- The `:267` edit is the ONLY edit outside the plan's three named ranges; recorded as deviation in this report and in the commit message body if any reader wants the audit trail.

## Commit

- Subject: `docs(aleph-hub): §7 keeps every ruling, drops the comparison framing` (conventional, English, scope `aleph-hub`; matches plan Step 4 subject verbatim).
- Trailer: `Co-Authored-By: MiniMax M3 <noreply@minimax.chat>` (per this dispatch's instruction; plan default `Claude Opus 5 (1M context) <noreply@anthropic.com>` not used).
- Files staged: `docs/reference/ALEPH_HUB.md` (explicit) + `.superpowers/sdd/2026-09-20-plugin-scope-and-cc-compat/task-P8.2-report.md` (forced add — `.superpowers/` is root-gitignored).
