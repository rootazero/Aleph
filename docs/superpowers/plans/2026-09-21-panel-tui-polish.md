# Panel & TUI Polish (Phase 0 + Phase 1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish a green test baseline (Phase 0) and produce a Tier-classified bug+wiring audit table (Phase 1) that will drive the fix plan in Plan 2.

**Architecture:** Read-only audit + test runs only. No production code modifications in this plan. Phase 1 produces `gap-analysis.md` as the bridge to Plan 2.

**Tech Stack:** Cargo workspace, vitest, Playwright, worktree-isolated git branch (`panel-tui-polish`).

**Spec:** `docs/superpowers/specs/2026-09-21-panel-tui-polish-design.md`

---

## Global Constraints

- **Worktree only**: work in `/home/zou/data/workspace/Aleph-panel-tui-polish` (branch `panel-tui-polish`); never touch `main`
- **Memory limits** (this machine <16GB):
  - Always: `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1`
  - For alephcore lib test, the single rustc process can consume 8GB+
- **Process management** (CRITICAL):
  ```bash
  pkill -f "target/release/aleph-server" 2>/dev/null
  pkill -f "target/debug/aleph-server" 2>/dev/null
  sleep 2
  ```
  Multiple `aleph-server` processes → HMAC failure → vault data loss
- **Commit format**: `<scope>: <description>` (English). Example: `gateway: add WebSocket server foundation`
- **Style**: rustfmt (4-space indent, 100 char width) + clippy (`-D warnings`)
- **Reference**: `FEATURE_LOCATOR.md` is the canonical status table; do not invent new anchors
- **No production code changes** in this plan (Phase 0/1 are read-only)

---

## Review Focus

This plan produces only test runs + audit documents. The five failure modes that could most likely bite a person using these outputs:

1. **Test false-positive baseline** — A test passes locally but actually exercises a no-op (e.g., `#[ignore]`'d, env-gated off). Result: baseline claims "green" but real code is broken.
   - Pinned in Task 1.6 (test runner integration check).

2. **Audit gap misses a category** — Grep + read may miss ❌ items already known to `FEATURE_LOCATOR.md`; or miss UX patterns visible only in reference projects.
   - Pinned in Task 2.4 (FEATURE_LOCATOR cross-check) and Task 2.5 (reference project sweep).

3. **Snapshot drift from working tree pollution** — Test results include uncommitted local changes that mask real regressions.
   - Pinned in Task 1.2 (clean working tree assertion).

4. **Process leak between Phase 0 and Plan 2** — If baseline test runs leave a long-lived `aleph-server`, Plan 2 will hit HMAC failures.
   - Pinned in Task 1.7 (post-run process check).

5. **Audit doc drift from spec scope** — Audit discovers gaps outside A/B/C/D; if those leak into gap-analysis, scope creeps.
   - Pinned in Task 2.8 (scope-strict gate at end of audit doc).

---

## Task 1: Phase 0 — Establish Green Test Baseline

**Files:**
- Create: `docs/superpowers/plans/phase0-baseline/test-results-baseline.json`
- Create: `docs/superpowers/plans/phase0-baseline/baseline-summary.md`
- (no production code modifications)

**Interfaces:**
- Consumes: current working tree at commit `0927461ba` (HEAD of `panel-tui-polish` branch)
- Produces:
  - `test-results-baseline.json`: machine-readable baseline (per-suite: pass/fail/skip counts + commit)
  - `baseline-summary.md`: human-readable summary with status per command, duration, and any pre-existing failures noted

- [ ] **Step 1.1: Verify clean working tree + correct branch**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git status --short
git branch --show-current
git rev-parse HEAD
```

Expected:
- `git status --short`: empty (clean)
- `git branch --show-current`: `panel-tui-polish`
- `git rev-parse HEAD`: `0927461ba...` (or current HEAD on the worktree branch)

If `git status --short` shows modifications, **stop** and ask the user how to proceed (do not stash, do not discard).

- [ ] **Step 1.2: Kill any running aleph-server (avoid HMAC leak)**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep -E "aleph-server" | grep -v grep
```

Expected: no `aleph-server` process. If any survive, escalate before continuing.

- [ ] **Step 1.3: Run alephcore lib test (memory-limited)**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
mkdir -p docs/superpowers/plans/phase0-baseline
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p alephcore --lib 2>&1 \
  | tee docs/superpowers/plans/phase0-baseline/alephcore-lib-test.log
```

Expected: tests run to completion (may take 5-15 min on this machine). Capture exit code via:
```bash
echo "exit: $?" >> docs/superpowers/plans/phase0-baseline/alephcore-lib-test.exit
```

If OOM killer triggers (look for "could not compile; N warnings emitted" disguises or kernel OOM messages in `dmesg`), re-run with even tighter settings:
```bash
CARGO_BUILD_JOBS=1 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p alephcore --lib --test test_name
```
and note in baseline-summary.md which test triggered OOM.

- [ ] **Step 1.4: Run webchat + tui lib tests**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p webchat --lib 2>&1 \
  | tee docs/superpowers/plans/phase0-baseline/webchat-lib-test.log
echo "exit: $?" >> docs/superpowers/plans/phase0-baseline/webchat-lib-test.exit

CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo test -p tui --lib 2>&1 \
  | tee docs/superpowers/plans/phase0-baseline/tui-lib-test.log
echo "exit: $?" >> docs/superpowers/plans/phase0-baseline/tui-lib-test.exit
```

Expected: both pass. (webchat + tui are smaller than alephcore, less likely to OOM.)

- [ ] **Step 1.5: Run pnpm vitest + Playwright e2e**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
pnpm install --frozen-lockfile 2>&1 | tee docs/superpowers/plans/phase0-baseline/pnpm-install.log
echo "exit: $?" >> docs/superpowers/plans/phase0-baseline/pnpm-install.exit

pnpm test 2>&1 | tee docs/superpowers/plans/phase0-baseline/vitest.log
echo "exit: $?" >> docs/superpowers/plans/phase0-baseline/vitest.exit

pnpm e2e 2>&1 | tee docs/superpowers/plans/phase0-baseline/playwright.log
echo "exit: $?" >> docs/superpowers/plans/phase0-baseline/playwright.exit
```

Expected:
- `pnpm install --frozen-lockfile`: exit 0
- `pnpm test` (vitest): all green
- `pnpm e2e` (Playwright): all green

If Playwright is not configured (no `playwright.config.ts` or it skips browser-required tests), note "e2e skipped by config" in baseline-summary.md.

- [ ] **Step 1.6: Verify tests are actually exercising code (catches false-positive baseline)**

Run a quick sanity grep to confirm at least one test per major target file:
```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
echo "=== chat_area tests ===" 
grep -l "fn test_" interfaces/tui/src/tui/widgets/chat_area.rs 2>/dev/null || echo "no inline tests in chat_area.rs (covered by app/tests.rs)"
grep -l "chat_area" interfaces/tui/src/tui/app/tests.rs

echo "=== messages tests ==="
grep -l "fn test_" interfaces/webchat/src/platform/wide/views/chat/messages.rs 2>/dev/null || echo "no inline tests (covered elsewhere)"
grep -rl "messages" interfaces/webchat/src/components/ | head -5

echo "=== app/mod.rs tests ==="
ls interfaces/tui/src/tui/app/tests.rs && wc -l interfaces/tui/src/tui/app/tests.rs
```

Expected: each major file has either inline `#[test]` or external test files exercising it. If a major module has zero test coverage, **note it in baseline-summary.md** as "uncovered module — Plan 2 must add tests before refactor".

- [ ] **Step 1.7: Confirm no orphan aleph-server**

```bash
pkill -f "target/release/aleph-server" 2>/dev/null
pkill -f "target/debug/aleph-server" 2>/dev/null
sleep 2
ps aux | grep -E "aleph-server" | grep -v grep
```

Expected: empty. If any process survives, log its PID + command line to baseline-summary.md.

- [ ] **Step 1.8: Build test-results-baseline.json + baseline-summary.md**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
COMMIT=$(git rev-parse HEAD)
DATE=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
cat > docs/superpowers/plans/phase0-baseline/test-results-baseline.json <<EOF
{
  "commit": "$COMMIT",
  "captured_at": "$DATE",
  "branch": "$(git branch --show-current)",
  "suites": {
    "alephcore_lib": {
      "exit": $(cat docs/superpowers/plans/phase0-baseline/alephcore-lib-test.exit),
      "log": "docs/superpowers/plans/phase0-baseline/alephcore-lib-test.log"
    },
    "webchat_lib": {
      "exit": $(cat docs/superpowers/plans/phase0-baseline/webchat-lib-test.exit),
      "log": "docs/superpowers/plans/phase0-baseline/webchat-lib-test.log"
    },
    "tui_lib": {
      "exit": $(cat docs/superpowers/plans/phase0-baseline/tui-lib-test.exit),
      "log": "docs/superpowers/plans/phase0-baseline/tui-lib-test.log"
    },
    "vitest": {
      "exit": $(cat docs/superpowers/plans/phase0-baseline/vitest.exit),
      "log": "docs/superpowers/plans/phase0-baseline/vitest.log"
    },
    "playwright": {
      "exit": $(cat docs/superpowers/plans/phase0-baseline/playwright.exit),
      "log": "docs/superpowers/plans/phase0-baseline/playwright.log"
    }
  }
}
EOF
```

Then write `baseline-summary.md` capturing:
- Which suites passed/failed and why
- OOM incidents (if any)
- Uncovered modules flagged from Step 1.6
- Orphan aleph-server status from Step 1.7
- Total wall-clock duration per suite
- Any pre-existing failures marked as "known broken at baseline"

If any suite fails, **stop and surface to user** before Phase 2 — Plan 2 cannot be written against a red baseline.

- [ ] **Step 1.9: Commit baseline artifacts**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add docs/superpowers/plans/phase0-baseline/
git commit -m "phase0: capture test baseline + summary doc (no code touched)"
```

---

## Task 2: Phase 1 — Audit + gap-analysis.md (Read-Only)

**Files:**
- Create: `docs/superpowers/plans/phase1-audit/gap-analysis.md`
- Create: `docs/superpowers/plans/phase1-audit/track-a-findings.md`
- Create: `docs/superpowers/plans/phase1-audit/track-b-findings.md`
- Create: `docs/superpowers/plans/phase1-audit/track-c-findings.md`
- Create: `docs/superpowers/plans/phase1-audit/track-d-findings.md`

**Interfaces:**
- Consumes:
  - Spec `docs/superpowers/specs/2026-09-21-panel-tui-polish-design.md` §3 (track A/B/C/D subitems)
  - `docs/reference/FEATURE_LOCATOR.md` (canonical ❌/⚠️ status table)
  - `test-results-baseline.json` (which tests exist + which modules are uncovered)
  - Reference projects (read-only):
    - `/home/zou/mnt/macmini/TBU4/Github/pi-ask-user/`
    - `/home/zou/mnt/macmini/TBU4/Github/pi-ask-user-question/`
    - `/home/zou/mnt/macmini/TBU4/Github/pi-claude-code-tui/`
    - `/home/zou/mnt/macmini/TBU4/Github/desktop-cc-gui/`
- Produces:
  - `gap-analysis.md`: aggregate of all 4 tracks, each item = anchor + symptom + fix sketch + Tier (1/2/3) + est. lines + test plan
  - `track-{a,b,c,d}-findings.md`: per-track detail supporting the gap-analysis aggregation

- [ ] **Step 2.1: Read FEATURE_LOCATOR.md §0 + §5 + §6 (Panel + UI + Desktop)**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
wc -l docs/reference/FEATURE_LOCATOR.md
```

Then read FEATURE_LOCATOR.md completely (use read with offset/limit — file is ~6000 lines, ~50KB; chunked to work).

For each entry marked ❌ or ⚠️, capture: anchor path + status + scope (which track it belongs to: A/B/C/D).

Output to `track-{a,b,c,d}-findings.md` §"FEATURE_LOCATOR 已知未完成项".

- [ ] **Step 2.2: Read FEATURE_LOCATOR.md Panel + TUI specific entries**

Focus on rows tagged "Panel" or "TUI" (or that mention chat_area, messages, events, run_phase, etc.).

Cross-reference with spec §3 subitems A1–A7, B1–B8, C1–C7, D1–D4.

For each spec subitem:
- Find the FEATURE_LOCATOR anchor that maps to it
- Mark ✅ / ⚠️ / ❌ based on FEATURE_LOCATOR
- If no anchor exists (gap), note as "no anchor — needs new entry in Phase 4"

Add this cross-reference table to each `track-X-findings.md`.

- [ ] **Step 2.3: Static analysis of uncovered modules (from baseline Step 1.6)**

For each module flagged "uncovered" in baseline-summary.md:
- Read the module (read with offset/limit — they're large)
- Identify likely-bug hot spots: `unwrap()` in production paths, missing error handling, race conditions, etc.
- Note in `track-X-findings.md` §"潜在 bug 热点"

Don't fix anything — just enumerate.

- [ ] **Step 2.4: Cross-check against reference project UX modes (pi-ask-user + pi-ask-user-question)**

```bash
ls /home/zou/mnt/macmini/TBU4/Github/pi-ask-user/
ls /home/zou/mnt/macmini/TBU4/Github/pi-ask-user-question/
```

Read these reference files:
- `pi-ask-user/README.md`
- `pi-ask-user/index.ts` (skim; it's the main impl)
- `pi-ask-user/skills/ask-user/SKILL.md`
- `pi-ask-user-question/README.md`
- `pi-ask-user-question/extensions/` (skim the package structure)

Extract UX patterns they implement:
- Split-pane details preview on wide terminals
- Multi-select option lists with descriptions
- Freeform responses
- Overlay toggle (`alt+o`)
- Inline vs overlay modes
- Persistent single-column preference
- `herdr:blocked` lifecycle events
- `details` payload for session state reconstruction
- Escape → "use best judgment" fallback
- `promptSnippet` + `promptGuidelines` injection

For each pattern, check Aleph's current implementation in:
- `src/clarification/ask.rs`
- `src/builtin_tools/ask_user.rs`
- `interfaces/tui/src/tui/widgets/dialog.rs`
- `interfaces/webchat/src/components/ask_user_card.rs`

Note gaps in `track-b-findings.md` §"pi-ask-user 模式差距".

- [ ] **Step 2.5: Cross-check against reference TUI impl (pi-claude-code-tui)**

```bash
ls /home/zou/mnt/macmini/TBU4/Github/pi-claude-code-tui/extensions/
```

Read:
- `pi-claude-code-tui/extensions/claude-code-tui.ts` (full)
- `pi-claude-code-tui/extensions/lib/claude-tui-editor.ts` (full)

Extract concrete impl patterns:
- Spinner frames (`["·","","✱","✶","✻","✽"]`) + 190 playful verbs + past-tense verbs for completion
- CC-style half-rounded input border + accent block cursor
- Tool row `⏺ Tool(args)` + `⎿  output` pattern with edit diff coloring
- Read tool: 3-line collapse on long output
- Spinner footer `✻ Worked for 12s` pattern
- Status bar pattern: `model │ Context 23% (50k/200k) │ $0.042`
- Native footer toggle (`/claude-footer`) for co-existence with other extensions

For each pattern, find Aleph's equivalent:
- TUI spinner: `interfaces/tui/src/tui/widgets/header.rs` + `theme.rs`
- TUI input area: `interfaces/tui/src/tui/widgets/input_area.rs`
- TUI tool row: `interfaces/tui/src/tui/widgets/tool_row.rs`
- TUI status bar: `interfaces/tui/src/tui/widgets/status_bar.rs`

Note gaps in `track-a-findings.md` §"pi-claude-code-tui 模式差距".

- [ ] **Step 2.6: Cross-check against reference Panel impl (desktop-cc-gui)**

```bash
ls /home/zou/mnt/macmini/TBU4/Github/desktop-cc-gui/src/features/
ls /home/zou/mnt/macmini/TBU4/Github/desktop-cc-gui/src/components/
```

Read the chat-related folders (`features/chat/`, `components/chat/`, etc.).

Extract concrete impl patterns:
- Composer attachment layout (drag/drop, paste, preview thumbnails)
- Message stream virtualization
- @mention palette UI
- Approval card UI (approve/deny buttons + reason)
- Reasoning fold/unfold UI
- Live turn indicator + elapsed timer

For each pattern, find Aleph's equivalent:
- Composer: `interfaces/webchat/src/platform/wide/views/chat/composer/{mod,attachments,palette,voice}.rs`
- Messages: `interfaces/webchat/src/platform/wide/views/chat/{messages,timeline,transcript}.rs`
- @mention: `mention_palette.rs`
- Approval: `tool_approvals.rs`
- Reasoning: `reasoning.rs`

Note gaps in `track-a-findings.md` and `track-b-findings.md` §"desktop-cc-gui 模式差距".

- [ ] **Step 2.7: Cross-check shared contracts (track D)**

Read these files to identify if there's any duplication / inconsistency between Panel and TUI:
- `shared/ui_logic/src/state/` (entire dir)
- `interfaces/webchat/src/platform/wide/views/chat/state/mod.rs`
- `interfaces/tui/src/tui/app/mod.rs` (state-related parts)
- `aleph_protocol/src/session_thread.rs` (relevant types)

For each shared concept (session_row, run_phase, message projection, keymap):
- Where is the source of truth?
- Where is it consumed?
- Are there inconsistencies between Panel and TUI (e.g., different field names, different enums)?

Note findings in `track-d-findings.md` §"契约差距".

- [ ] **Step 2.8: Aggregate to gap-analysis.md with scope-strict gate**

Create `docs/superpowers/plans/phase1-audit/gap-analysis.md` with structure:

```markdown
# gap-analysis.md — Panel & TUI Polish (Phase 1 审计输出)

> 关联 spec: docs/superpowers/specs/2026-09-21-panel-tui-polish-design.md
> 基线: docs/superpowers/plans/phase0-baseline/test-results-baseline.json
> 生成时间: <ISO8601>

## 摘要
- 总条目数: <N>
- Tier-1（用户最直接感受）: <N1>
- Tier-2（状态/事件正确性）: <N2>
- Tier-3（性能/样式，本轮跳过）: <N3>

## Tier-1 清单（待 Phase 2 修复）
每条格式：| Anchor | 现象 | 修复建议 | 估计行数 | 测试 |

## Tier-2 清单（待 Phase 3 修复）
（同上）

## Tier-3 清单（本轮跳过，下轮重构）
（同上）

## 范围闸（scope gate）
- 每条是否落在 A/B/C/D 四个赛道之一？是 → 保留；否 → 移出本轮
- 每条是否在 L1 范围（仅修 bug+连线，不重写）？否 → 移出本轮
```

Scope-strict gate: every item must map to a spec subitem (A1-A7, B1-B8, C1-C7, D1-D4). Items that don't fit the spec are explicitly out of scope.

- [ ] **Step 2.9: Commit audit artifacts**

```bash
cd /home/zou/data/workspace/Aleph-panel-tui-polish
git add docs/superpowers/plans/phase1-audit/
git commit -m "phase1: gap-analysis.md + per-track findings (audit only, no code changes)"
```

---

## Task 3: User Review Gate

**Files:** (no file ops)

**Interfaces:**
- Produces: user-approval signal → Plan 2 creation gate

- [ ] **Step 3.1: Surface gap-analysis to user**

Show user:
- Path: `docs/superpowers/plans/phase1-audit/gap-analysis.md`
- Summary: counts per Tier
- Key Tier-1 items (top 5 most user-visible)

- [ ] **Step 3.2: Ask user to review + adjust scope**

Use `ask_user` to get explicit yes/no on whether to proceed to Plan 2.

Options to present:
- "Approve as-is → proceed to Plan 2 with this list"
- "Drop Tier-1 items X, Y (too risky / not worth it)"
- "Add Tier-1 items not in audit"
- "Defer entire audit to next round"

If user requests changes, update gap-analysis.md, recommit.

---

## Task 4: Hand off to Plan 2 (Out of Scope Here)

After Task 3 approval:

- [ ] **Step 4.1: Acknowledge plan boundary**

Print: "Plan 1 complete. Plan 2 (Phase 2/3/4: fix + verify + docs) will be authored separately, against the audited Tier-1 + Tier-2 list, and reviewed by you before execution."

- [ ] **Step 4.2: Stop**

Do not begin Plan 2 without explicit user request after this plan completes.