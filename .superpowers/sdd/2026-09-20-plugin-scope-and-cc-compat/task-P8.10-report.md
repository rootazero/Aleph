# Task P8.10 Report — One archive at `docs/archive/` for the 2026-03-18 ClawHub spec + plan and the voice-sidecar file (2026-09-20)

- **Plan**: `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1957-2005`
- **Files modified**: 4 (`docs/archive/2026-03-18-clawhub-integration-design.md` = rename+modify; `docs/archive/2026-03-18-clawhub-integration-plan.md` = rename+modify; `docs/archive/SELF_BUILT_VOICE_SIDECAR.md` = rename+modify; `docs/reference/FEATURE_LOCATOR.md` = drift-fix modification). No code touched.
- **HEAD at start**: `53b05e93f` clean
- **HEAD at end**: `53b05e93f + 1 commit` clean after commit (see "Commit" section below)

## Change

Four docs changes (3 git-tracked renames with banner prepends + 1 drift fix):

1. **(a) `git mv docs/superpowers/specs/2026-03-18-clawhub-integration-design.md` → `docs/archive/2026-03-18-clawhub-integration-design.md`** (rename + content modify). Banner prepended as first line (above the existing `# ClawHub Integration Design` heading). Banner content (verbatim from plan lines 1979–1983):

   > **ARCHIVED 2026-09-20** — historical. The ClawHub integration this describes was never built as described (`src/clawhub/` never existed; the SKILL.md `metadata.openclaw.*` DTOs it specified were parsed, never read, and CUT 2026-09-20). Kept for provenance only. Do not implement from it.

2. **(b) `git mv docs/superpowers/plans/2026-03-18-clawhub-integration.md` → `docs/archive/2026-03-18-clawhub-integration-plan.md`** (rename + content modify). Banner prepended (same verbatim text as (a)). The `-plan` suffix on the basename (per plan naming convention recorded at P5P8-cut-and-docs.md:1966) distinguishes the two files side by side.

3. **(c) `git mv docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md` → `docs/archive/SELF_BUILT_VOICE_SIDECAR.md`** (rename + content modify). Banner prepended (verbatim from plan lines 1987–1990):

   > **ARCHIVED** — moved from `docs/reference/archive/` on 2026-09-20 so the repo has one archive (`docs/archive/`, CLAUDE.md Tier 3). Content unchanged: a preserved design, kept for future revival (commit `af2fe5a5e`).

4. **(d) `docs/reference/FEATURE_LOCATOR.md` drift fix** (one-line edit at line 1241). The pre-existing `⑥ CUT 清单（熵减）` round entry contained the literal `\`docs/superpowers/{specs,plans}/2026-03-18-clawhub-integration*\`` (a glob describing the old superpowers location). This was a P8.7-introduced inbound link that became stale after this task's moves; per plan Step 1 "if a link appeared since `3ddc1f2e7`, fix it in this commit". Changed the path in-place to `\`docs/archive/2026-03-18-clawhub-integration*\``. Verb `归档` preserved (now reads as a past-tense record of the move).

After (a)+(b)+(c): `rmdir docs/reference/archive` succeeded — the old archive directory was empty and is removed from the tree.

## Pre-condition (plan Step 1)

```
$ ls docs/archive
ls: docs/archive: No such file or directory

$ git ls-files docs/superpowers/specs/2026-03-18-clawhub-integration-design.md \
                 docs/superpowers/plans/2026-03-18-clawhub-integration.md \
                 docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md
docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md
docs/superpowers/plans/2026-03-18-clawhub-integration.md
docs/superpowers/specs/2026-03-18-clawhub-integration-design.md
```

- `docs/archive` does not exist. ✓ (Plan required "No such file".)
- All three files listed in `git ls-files`. ✓ (Plan required "all three listed".)

**Inbound-link grep at HEAD `53b05e93f` (plan Step 1 — re-run to find new refs since `3ddc1f2e7`):**

```
$ rg -n '2026-03-18-clawhub-integration' . --glob '!node_modules'
./docs/reference/FEATURE_LOCATOR.md:1241:    …`docs/superpowers/{specs,plans}/2026-03-18-clawhub-integration*` 归档…
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1737:    …`docs/superpowers/{specs,plans}/2026-03-18-clawhub-integration*` 归档…
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1962:- Move: `docs/superpowers/specs/2026-03-18-clawhub-integration-design.md` → …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1963:- Move: `docs/superpowers/plans/2026-03-18-clawhub-integration.md` → …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1968:- [ ] **Step 1: Pre-condition** — … git ls-files docs/superpowers/specs/2026-03-18-clawhub-integration-design.md docs/superpowers/plans/2026-03-18-clawhub-integration.md …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1974:git mv docs/superpowers/specs/2026-03-18-clawhub-integration-design.md …
./docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-design.md:213:| OpenClaw 文档 | …`docs/superpowers/{specs,plans}/2026-03-18-clawhub-integration*` 移 `docs/archive/` | …
./docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-evidence/scan-aleph-plugins.md:195:| `docs/superpowers/specs/2026-03-18-clawhub-integration-design.md`, `docs/superpowers/plans/2026-03-18-clawhub-integration.md` | … | Archive (Tier 3) |
./docs/superpowers/plans/2026-03-18-clawhub-integration.md:11:**Spec:** `docs/superpowers/specs/2026-03-18-clawhub-integration-design.md`
```

- **`docs/reference/FEATURE_LOCATOR.md:1241` — IN SCOPE for drift fix** (a P8.7-introduced inbound link, not the current plan and not the historical plan). This is the one the plan required fixing in this commit.
- **`docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1737` — current plan mirror of the same CUT-list bullet**. NOT modified: per user instruction "不要修改历史/计划自引用" and per plan's own exclusion "outside the two files and the 2026-09-20 spec/evidence → 0" (the plan is an allowed exclusion).
- **`docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1962, 1963, 1968, 1974` — current plan self-references (the Step 2 / Step 1 / `git mv` text)**. NOT modified: per user instruction.
- **`docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-design.md:213`** and **`docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-evidence/scan-aleph-plugins.md:195` — current spec / evidence**. Per plan Step 1 exclusion ("outside the two files and the 2026-09-20 spec/evidence → 0"), NOT modified; the plan's own drift budget explicitly carves out these two as not-fixable-in-this-commit. (And the user instruction scope is explicitly **only** FEATURE_LOCATOR.md.)
- **`docs/superpowers/plans/2026-03-18-clawhub-integration.md:11` — the file being moved; its internal `**Spec:**` link**. Per plan "Content unchanged" (P5P8-cut-and-docs.md:1985–1987), banner is the only edit; this internal link stays as-is. After the move, this internal link becomes an **unreachable literal** in the archived plan — recorded below in Deviations §1.

```
$ rg -n 'SELF_BUILT_VOICE_SIDECAR|reference/archive' . --glob '!node_modules'
./docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md:1:# 自建语音引擎 Sidecar（aleph-voice）设计存档
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/RECONCILIATION.md:152:  `docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md` there, deletes `docs/reference/archive/`, and greps for
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1957:### Task P8.10: One archive at `docs/archive/` …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1959:> **Reconciled (R5.2 Q7):** ONE archive location … `docs/reference/archive/` …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1964:- Move: `docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md` → …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1966:**Naming convention (recorded, since the directory is new):** …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1968:- [ ] **Step 1: Pre-condition** — … docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1976:git mv docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1990:> **ARCHIVED** — moved from `docs/reference/archive/` on 2026-09-20 …
./docs/superpowers/plans/2026-04-22-whatsapp-arch-r1.md:694:git mv docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md docs/reference/archive/WHATSAPP_ARCHITECTURE_DESIGN-2026-04-06.md
```

- **`docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md:1` — the file being moved; its own title**. Self-excluded.
- **`docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/RECONCILIATION.md:152`** and **`P5P8-cut-and-docs.md:1957, 1959, 1964, 1966, 1968, 1976, 1990`** — current plan (and its sibling reconciliation doc). NOT modified: per user instruction "不要修改历史/计划自引用" + per user instruction "排除当前 plan".
- **`docs/superpowers/plans/2026-04-22-whatsapp-arch-r1.md:694`** — pre-existing whatsapp historical plan referencing the (now-empty) `docs/reference/archive/` directory. NOT modified: per user instruction "排除既存 whatsapp 历史计划".

After applying user-mandated excludes (`docs/archive`, current plan, whatsapp historical plan), the live references that required fixes in this commit = **1** (`FEATURE_LOCATOR.md:1241`).

## Post-condition (plan Step 3)

```
$ git status --short docs/
RM docs/superpowers/specs/2026-03-18-clawhub-integration-design.md -> docs/archive/2026-03-18-clawhub-integration-design.md
RM docs/superpowers/plans/2026-03-18-clawhub-integration.md -> docs/archive/2026-03-18-clawhub-integration-plan.md
RM docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md -> docs/archive/SELF_BUILT_VOICE_SIDECAR.md
 M docs/reference/FEATURE_LOCATOR.md

$ git status --short docs/ | grep -c '^R'
3
```

- Three `R` (rename + modify due to banner) lines for the three moves. ✓ (Plan required "three `R` lines". Status is `RM` not pure `R` because the banner prepend counts as a content change; this is git's standard rename-with-modify form and the same shape as documented for similar archive-move commits.)

```
$ rg -n 'ARCHIVED' docs/archive | wc -l
       3
```

- ARCHIVED banner present in all three files (one banner per file). ✓ (Plan required 3.)

```
$ ls docs/superpowers/specs docs/superpowers/plans | grep -c clawhub
0
```

- `clawhub` files no longer present in old paths. ✓ (Plan required 0.)

```
$ ls docs/reference/archive
ls: docs/reference/archive: No such file or directory
```

- Old archive directory removed. ✓ (Plan required "No such file".)

```
$ ls docs/archive
2026-03-18-clawhub-integration-design.md
2026-03-18-clawhub-integration-plan.md
SELF_BUILT_VOICE_SIDECAR.md
```

- New archive directory contains exactly the three moved files. ✓

```
$ rg -n 'reference/archive' . --glob '!node_modules'
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/RECONCILIATION.md:152:  `docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md` there, deletes `docs/reference/archive/`, and greps for
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1957: …, and the voice-sidecar file from `docs/reference/archive/`
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1959: … `docs/reference/archive/` (one file, `SELF_BUILT_VOICE_SIDECAR.md`, commit `af2fe5a5e`) …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1964:- Move: `docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md` → `docs/archive/SELF_BUILT_VOICE_SIDECAR.md` …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1966: … `rg -n 'SELF_BUILT_VOICE_SIDECAR|reference/archive' .` outside the file itself → 0 (verified) …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1968:- [ ] **Step 1: Pre-condition** — … docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1976:git mv docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md …
./docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/P5P8-cut-and-docs.md:1990:> **ARCHIVED** — moved from `docs/reference/archive/` on 2026-09-20 …
./docs/superpowers/plans/2026-04-22-whatsapp-arch-r1.md:694:git mv docs/reference/WHATSAPP_ARCHITECTURE_DESIGN.md docs/reference/archive/WHATSAPP_ARCHITECTURE_DESIGN-2026-04-06.md
```

- Plan's literal post-condition `rg -n 'reference/archive' . --glob '!node_modules'` → 0 is **not literally 0** in practice — the post-move repo still has references in current plan + whatsapp historical plan (which the user explicitly excluded). **Deviation**: this literal is unattainable in any commit that doesn't rewrite the plan itself (which user instruction forbids). The user-mandated excluded-set count is **0** (re-confirmed below).

**Live-ref re-check after user-mandated excludes:**

```
$ rg -n '2026-03-18-clawhub-integration' . \
     --glob '!docs/archive' \
     --glob '!docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/**' \
     --glob '!docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-design.md' \
     --glob '!docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-evidence/**' \
     --glob '!docs/superpowers/plans/2026-04-22-whatsapp-arch-r1.md' \
     --glob '!node_modules'
./docs/reference/FEATURE_LOCATOR.md:1241:    …`docs/archive/2026-03-18-clawhub-integration*` 归档…
```

```
$ rg -n 'reference/archive|SELF_BUILT_VOICE_SIDECAR' . \
     --glob '!docs/archive' \
     --glob '!docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/**' \
     --glob '!docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-design.md' \
     --glob '!docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-evidence/**' \
     --glob '!docs/superpowers/plans/2026-04-22-whatsapp-arch-r1.md' \
     --glob '!node_modules'
(no output; EXIT=1)
```

- **ClawHub live refs (post-exclude)**: 1 hit — `FEATURE_LOCATOR.md:1241`, and the literal in that line now correctly points to `docs/archive/2026-03-18-clawhub-integration*` (REACHABLE — matches both `2026-03-18-clawhub-integration-design.md` and `2026-03-18-clawhub-integration-plan.md` in the new archive). ✓
- **`reference/archive` / `SELF_BUILT_VOICE_SIDECAR` live refs (post-exclude)**: 0 hits. ✓ (User instruction: "记录不可达字面 0 deviation".)
- **Total live inbound refs to fix in this commit**: 1 (`FEATURE_LOCATOR.md:1241`). Fixed. ✓

## Banner content verification (per `rg -n 'ARCHIVED' docs/archive`)

| File | Banner first line |
|---|---|
| `docs/archive/2026-03-18-clawhub-integration-design.md` | `> **ARCHIVED 2026-09-20** — historical. The ClawHub integration this describes was never built as described (\`src/clawhub/\` never existed; the SKILL.md \`metadata.openclaw.*\` DTOs it specified were parsed, never read, and CUT 2026-09-20). Kept for provenance only. Do not implement from it.` |
| `docs/archive/2026-03-18-clawhub-integration-plan.md` | (same as above) |
| `docs/archive/SELF_BUILT_VOICE_SIDECAR.md` | `> **ARCHIVED** — moved from \`docs/reference/archive/\` on 2026-09-20 so the repo has one archive (\`docs/archive/\`, CLAUDE.md Tier 3). Content unchanged: a preserved design, kept for future revival (commit \`af2fe5a5e\`).` |

Both banners are **verbatim** from the plan (lines 1985–1987 for clawhub, lines 1990–1991 for voice-sidecar), prepended above the existing title in each file. No paraphrase, no abbreviation.

## File-size preservation check

```
$ git show HEAD:docs/superpowers/plans/2026-03-18-clawhub-integration.md | wc -l
    1634
$ wc -l docs/archive/2026-03-18-clawhub-integration-plan.md
    1636 docs/archive/2026-03-18-clawhub-integration-plan.md

$ git show HEAD:docs/superpowers/specs/2026-03-18-clawhub-integration-design.md | wc -l
     442
$ wc -l docs/archive/2026-03-18-clawhub-integration-design.md
     444 docs/archive/2026-03-18-clawhub-integration-design.md

$ git show HEAD:docs/reference/archive/SELF_BUILT_VOICE_SIDECAR.md | wc -l
     324
$ wc -l docs/archive/SELF_BUILT_VOICE_SIDECAR.md
     326 docs/archive/SELF_BUILT_VOICE_SIDECAR.md
```

- Each file: original line count + 2 (banner line + blank separator). ✓ Content preserved past banner.

## Drift-fix verification (FEATURE_LOCATOR.md)

**Before:**
```
…`docs/superpowers/{specs,plans}/2026-03-18-clawhub-integration*` 归档…
```

**After (line 1241):**
```
…`docs/archive/2026-03-18-clawhub-integration*` 归档…
```

- Old superpowers path replaced with new docs/archive path. ✓
- Verb `归档` preserved (now reads as past-tense record of the move).
- Edit was a single-line replacement (the `oldText` was unique in the file — verified by `rg -c` that the exact 12-byte-prefix-of-old-text occurs exactly once).
- `git diff --check docs/reference/FEATURE_LOCATOR.md` → empty (no whitespace / conflict-marker warnings).
- `git diff --stat docs/reference/FEATURE_LOCATOR.md` → 1 file changed, 1 insertion, 1 deletion (no other lines touched in the file).

## Deviations from plan

1. **`rg -n 'reference/archive' . --glob '!node_modules'` is not literally 0** (plan Step 3 post-condition). The literal `rg` returns 9 hits after the move — all in `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/{RECONCILIATION.md, P5P8-cut-and-docs.md}` (the current plan + its sibling reconciliation doc, both recording the move as a fact) and `docs/superpowers/plans/2026-04-22-whatsapp-arch-r1.md` (a pre-existing historical plan). The plan's own expected 0 was written at `3ddc1f2e7` when those files didn't yet exist; it didn't anticipate that the plan itself would naturally contain references to its own move. **Action**: per user instruction, those files are excluded (current plan + whatsapp historical plan); user-mandated excluded-set count is **0** (verified above). No code change needed.

2. **FEATURE_LOCATOR.md drift-fix interpretation.** The plan's Step 1 says "if a link appeared since `3ddc1f2e7`, fix it in this commit" — at `53b05e93f`, exactly one such link exists (`FEATURE_LOCATOR.md:1241`, introduced by P8.7 in the same round). The plan's text doesn't specify the exact replacement form. User instruction: "修复 ... glob 为 docs/archive 路径" (replace the glob with the docs/archive path). I replaced the path component in-place (`docs/superpowers/{specs,plans}/2026-03-18-clawhub-integration*` → `docs/archive/2026-03-18-clawhub-integration*`), preserving the verb `归档`. Alternative interpretations (append `→ docs/archive/...`; rewrite as `移 docs/archive/...`) would have preserved the plan-template shape but left a stale inbound literal to a non-existent file; the chosen form is the cleanest "0 unreachable literal deviation" outcome.

3. **Status `RM` instead of pure `R`.** Plan Step 3 says "three `R` lines"; `git status --short docs/` shows three `RM` lines. This is git's standard rename-with-modify form (the `M` is the banner prepend counting as a content change). The shape is functionally identical to `R` for rename detection; `git status --short docs/ | grep -c '^R'` = 3 (the `R` prefix matches both `R` and `RM`). ✓

4. **Internal Spec link inside archived plan now points to a non-existent path.** `docs/archive/2026-03-18-clawhub-integration-plan.md:11` retains the verbatim line `**Spec:** \`docs/superpowers/specs/2026-03-18-clawhub-integration-design.md\`` (the original path, which no longer exists post-move). The plan's Step 2 explicitly says "Content unchanged" for the archived plan except for the banner; the user instruction "记录内部 archive plan 的旧 Spec link 未改（计划要求保留内容）" makes this deliberate. **Unreachable literal count post-commit: 1** (this line), up from 0 pre-commit. Not a regression — the plan required it.

5. **Current spec (`docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-design.md:213`) and evidence (`docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-evidence/scan-aleph-plugins.md:195`) still reference the old superpowers paths.** These are not modified. The plan's own Step 1 grep budget explicitly excludes them ("outside the two files and the 2026-09-20 spec/evidence → 0"), and the user instruction scope is "**only** FEATURE_LOCATOR.md". They will be addressed in the spec/evidence round (P8.x follow-up, not P8.10). Recorded here for completeness, not as a deviation in this commit.

## What was NOT done (state-the-negative, per AGENTS.md §6)

- No source file under `src/`, `tests/`, `interfaces/`, `shared/`, `qa/`, or `crates/` was modified. (`git diff HEAD -- src/` is empty.)
- No `docs/reference/{HARNESS_PHILOSOPHY,PLUGIN_SYSTEM,EXTENSION_SYSTEM,GATEWAY,ARCHITECTURE,ALEPH_HUB,SKILL_MODEL_TAXONOMY}.md` was touched — those are P8.1–P8.6 / P8.8+ territory.
- No `CLAUDE.md` was touched (P8.8 territory).
- No `qa/README.md` was touched (P8.9 territory).
- No `docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-design.md` was touched (current spec — explicitly excluded by plan Step 1 grep budget + user instruction scope; future round).
- No `docs/superpowers/specs/2026-09-20-plugin-scope-and-cc-compat-evidence/*` was touched (evidence — same rationale).
- No `docs/superpowers/plans/2026-09-20-plugin-scope-and-cc-compat/{RECONCILIATION,P5P8-cut-and-docs}.md` was touched (current plan + reconciliation — explicitly excluded by user instruction "不要修改历史/计划自引用").
- No `docs/superpowers/plans/2026-04-22-whatsapp-arch-r1.md` was touched (existing whatsapp historical plan — explicitly excluded by user instruction).
- No `docs/superpowers/plans/2026-03-18-clawhub-integration.md:11` (`**Spec:**` link inside the file) was modified — preserved per plan "Content unchanged" rule.
- No banner content was paraphrased — both banners are verbatim from P5P8-cut-and-docs.md:1985–1991.
- No `~/.claude/settings.json` was read; no `~/.claude/.aleph` was written; no `aleph-server` was started.
- No `cargo fmt`, no `--no-verify`, no `git add -A`, no `git stash`. `git add` was called with explicit paths only (`docs/archive`, `docs/superpowers/specs`, `docs/superpowers/plans`, `docs/reference/archive` for the moves; `docs/reference/FEATURE_LOCATOR.md` for the drift fix; `.superpowers/sdd/.../task-P8.10-report.md` with `-f` for this report).
- Did **not** run `cargo check` / `cargo test` / `cargo clippy` — no Rust change to validate, P8.10 is a pure docs task.
- Did **not** advance to P8.11 or any later task. P8.11+ is its own dispatch.
- Did **not** introduce new unreachable literals outside the one required by the plan ("Content unchanged" rule for the archived plan's internal Spec link).

## Commit

```
docs: one archive at docs/archive/ for dead designs

Co-Authored-By: MiniMax M3 <noreply@minimax.chat>
```

- Conventional-commit form, English, no scope (multi-file docs move + one drift fix; per dispatch instruction's verbatim message "docs: one archive at docs/archive/ for dead designs").
- Trailer: `Co-Authored-By: MiniMax M3 <noreply@minimax.chat>` (per this dispatch's instruction; plan default `Claude Opus 5 (1M context) <noreply@anthropic.com>` not used — model substitution recorded).
- Files staged explicitly (no `git add -A`, no `git add .`):
  - `docs/archive` (new directory + 3 files via the three prior `git mv` operations)
  - `docs/superpowers/specs` (the moved-out clawhub design file's old path)
  - `docs/superpowers/plans` (the moved-out clawhub plan file's old path)
  - `docs/reference/archive` (the removed old archive directory)
  - `docs/reference/FEATURE_LOCATOR.md` (the drift-fix modification)
  - `.superpowers/sdd/2026-09-20-plugin-scope-and-cc-compat/task-P8.10-report.md` (this report, added with `git add -f` because `.superpowers/` is gitignored at `.gitignore:107`)
- `git status` clean after commit.