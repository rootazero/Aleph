---
name: graph-review
description: Audit Aleph's knowledge graph (`.understand-anything/knowledge-graph.json`) for code-health signals — orphan nodes, dead-code candidates, R1 brain-limb separation violations, R10 thin-harness budget overrun. Use when the user types `/graph-review`, asks to "audit the graph", "check redlines", "find orphans", "find dead code", "verify thin harness", or wants a code-health pulse before a release / refactor / merge. Requires the graph file to exist — run `/understand` first if missing. Wraps `scripts/graph-audit.mjs` (no LLM cost, pure deterministic JSON walk).
---

# graph-review

Run `scripts/graph-audit.mjs` to surface code-health signals derived from `.understand-anything/knowledge-graph.json`. Default to the `summary` rollup; drill into individual audits only when a check is non-zero or the user asks for specifics.

This skill is **deterministic**: it reads a static JSON and emits counts/lists. No LLM call inside. Use the model's judgment only for **interpreting** the output and recommending follow-ups.

## Preconditions

- `.understand-anything/knowledge-graph.json` must exist at the repo root. If absent, tell the user: "No knowledge graph found. Run `/understand` first." Do NOT try to regenerate it inside this skill — `/understand` is a separate multi-phase pipeline.
- `node` available on PATH (already required by the project).

## Workflow

1. **Rollup first** — get one-line counts per check:
   ```bash
   node scripts/graph-audit.mjs summary
   ```
2. **Decide drill-in scope** based on rollup:
   - `orphans > 0` → run `orphans`. After Phase 7 cohort-wiring this should always be 0; a non-zero means `/understand` was interrupted or the graph is stale.
   - `dead-code > 0` is expected and noisy — only drill if user explicitly asks. Always preface with the under-capture caveat (see Caveats).
   - `redline-r1 > 0` → run `redline-r1`. Every hit is a real R1 violation (sandbox bridges already excluded).
   - `redline-r10 > 0` → run `redline-r10`. Means `src/harness/` exceeded 12 files; check whether new file is justified per CLAUDE.md R10 "3 questions before adding code".
3. **Drill in** with the relevant subcommand and `--limit=N` to widen lists:
   ```bash
   node scripts/graph-audit.mjs redline-r1 --limit=50
   node scripts/graph-audit.mjs dead-code --json | jq '.items[] | select(.file | startswith("src/gateway/"))'
   ```
4. **Cross-check before recommending fixes** — especially for dead-code (see Caveats). Use `rg`, `cargo machete`, or IDE find-usages.

## Commands

| Command | Output | Notes |
|---|---|---|
| `summary` | One-line counts per audit | Default; safe and fast |
| `orphans` | Nodes with zero edges | Expected: 0. Non-zero = stale or broken graph |
| `dead-code` | function/class nodes with no inbound `calls` edge | **CANDIDATES only** — see Caveats |
| `redline-r1` | `src/` files naming platform-native APIs (AppKit / Vision / win32 / objc / cocoa / etc.) | Violates R1 brain-limb separation. `src/sandbox/` + `src/bin/aleph-server/` excluded (legitimate bridges) |
| `redline-r10` | `src/harness/` `.rs` file count vs. 12-file budget | R10 thin-harness invariant — see CLAUDE.md §R10 |

Flags: `--json` (machine output, pipeable to `jq`), `--limit=N` (default 30, widens lists in human mode), `--graph=path` (point at a non-default graph file — useful for subdomain graphs like `frontend-knowledge-graph.json`).

## Caveats

- **dead-code is severely under-captured.** The graph currently captures ~4.5% of true call edges (~159 calls edges across ~3518 function nodes). The dead-code report will show thousands of false positives. Treat output as **candidates requiring manual verification**, not a deletion list. Always say so when reporting. The fix lives upstream in `/understand`'s file-analyzer subagent, not here.
- **redline-r1 token list is conservative.** Currently tokens like `use objc::`, `use cocoa::`, `use core_graphics::`, `use core_foundation::`, `NSWorkspace`, `AVCaptureDevice`, `CGEvent`, `use windows::`, `use winapi::`, `CheckNetIsolationEnableLoopback`, `CreateProcessW`. Add new tokens as new platform APIs appear in the codebase (edit the `NATIVE_API_TOKENS` array in `scripts/graph-audit.mjs`).
- **redline-r10 file budget is wired to CLAUDE.md §R10.** Current budget: 12 files. If CLAUDE.md is bumped, update `HARNESS_BUDGET` in `scripts/graph-audit.mjs` in the same commit — the two must stay in sync.
- **The script writes nothing.** Pure read. Safe to run repeatedly, including in CI.

## Reporting back to the user

After running, summarise in **3-5 lines max**:
- Which checks passed (one bullet, group them).
- Which need attention (one bullet per non-zero check, with the count).
- Top 1-2 candidates worth investigating, if any (with file path).

Do NOT dump full lists into the response. If listing is genuinely needed (e.g., user explicitly asks "show me all R1 violations"), prefer `--limit=5` and offer `--limit=N` to widen.

For R1 / R10 hits, link the redline in `CLAUDE.md`:
- **R1** — brain-limb separation: no platform-native API calls outside `src/sandbox/` (OS isolation bridge) and `src/bin/aleph-server/` (libc/process control).
- **R10** — thin-harness budget: `src/harness/` ≤ 12 files / ~4900 LOC. Adding a file requires answering the "3 questions before adding code" in CLAUDE.md.

## When NOT to use this skill

- User asks "who calls function X?" or "what does file Y depend on?" — this skill doesn't do per-symbol drill-down. Suggest `rg`, IDE find-usages, or future MCP server.
- User wants the dashboard visualisation — invoke `understand-anything:understand-dashboard` instead.
- Graph is stale (commits since last `/understand`) — re-run `/understand` first; this skill audits whatever's on disk and won't warn you that the graph is old.
