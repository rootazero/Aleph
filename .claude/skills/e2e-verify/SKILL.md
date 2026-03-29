---
name: e2e-verify
description: >
  E2E verification against a live server with intent-aware scenario design and
  multi-probe evidence chains. Use when: (1) user says "verify", "e2e test",
  "production verify", "validate module", "生产验证", "验证模块", (2) after
  completing a major feature that adds endpoints, tools, or RPC methods, (3) user
  runs /e2e-verify [target]. Analyzes git diff + user instructions to design
  positive/negative/boundary scenarios, manipulates environment to construct
  failure conditions, verifies with log + API + state + process probes.
  Flags: --debug (verbose output), --skip-build (reuse last binary).
---

# E2E Verify

Eight steps. No skipping.

| # | Step | What happens |
|---|------|-------------|
| 0 | Intent Analysis | User instructions + git diff + commit messages → test target list |
| 1 | Kill | Kill old server processes |
| 2 | Build | Compile (skip with `--skip-build`) |
| 3 | Start | Start with **elevated log level**, capture stdout/stderr to temp file |
| 4 | Verify | Process alive + port listening + auth (use `scripts/` client if available) |
| 5 | Scenario Design | Test cards: positive + negative + boundary per target |
| 6 | Execute | Backup → manipulate → test → collect probes → restore |
| 7 | Report | Evidence-based summary table |

## Step 0: Intent Analysis

Cross-reference three sources. Output a test target list.

**Source 1 — User instructions**: Parse `/e2e-verify provider fallback` for explicit targets. No args → rely on sources 2+3.

**Source 2 — Git diff**: `git diff HEAD~1..HEAD`. If empty, expand N until non-empty or prompt user.
Extract: new/modified functions (→ test targets), new error handling (→ **negative scenarios required**), new config/branches (→ **boundary scenarios required**), new tracing (→ available probes).

**Source 3 — Commit messages and existing tests**: Understand intent, find covered vs uncovered paths.

**Output format:**
```
Test Targets:
1. [positive] Normal request completes — source: user + diff
2. [negative] Fallback on primary failure — source: diff (new error handling)
3. [boundary] All options exhausted → graceful error — source: diff (conditional)
```

**Negative scenarios are NOT optional.** If diff contains error handling / fallback / degradation logic, corresponding negative scenarios are mandatory.

## Steps 1–4: Infrastructure

Discover project specifics from CLAUDE.md, README, Makefile/justfile/package.json. Check `scripts/` and `references/` in this skill's directory for existing tools.

**Critical details only:**
- **Step 1**: Read CLAUDE.md for process management rules before killing anything.
- **Step 3**: Start with elevated log level (`RUST_LOG=debug` / `LOG_LEVEL=DEBUG` / equivalent). Redirect stdout+stderr to `/tmp/e2e_server.log`. Record original level for restore.
- **Step 4**: If `scripts/` has a client library, use it. Otherwise: `kill -0 $PID` + port check + optional auth.

## Step 5: Scenario Design

For each target, produce a **test card**:

```
Target: [negative] Fallback on primary provider failure
Preconditions:
  - Backup: config.toml → config.toml.e2e.bak
  - Manipulate: Change default provider API key to invalid value
Trigger: Send chat request requiring LLM response
Expected:
  - Log probe: "fallback" or "retry" pattern in logs
  - API probe: completed=true, text non-empty
Restore: config.toml.e2e.bak → config.toml
```

**Mandatory scenario rules:**

| Type | Required when | Example |
|------|--------------|---------|
| Positive | Always (≥1 per target) | Normal request completes |
| Negative | Diff has error handling / fallback / degradation | Primary fails → fallback triggers |
| Boundary | Diff has conditionals / thresholds / null handling | All exhausted → graceful error |

**Enforcement** — if diff adds any of these patterns, negative/boundary scenarios are mandatory:
- `fallback`, `retry`, `timeout`, `degrade`, `error`, `recover` (all languages)
- Rust: `?`, `unwrap_or`, `match Err`, `Option::None`
- Python: `except`, `try/finally`, `raise`
- Go: `if err != nil`
- JS/TS: `.catch()`, `try/catch`, `??`

If diff genuinely has no error handling paths, only positive scenarios needed. Do NOT fabricate negative scenarios.

## Probe System

| Probe | Method | Best for |
|-------|--------|----------|
| **Log** | grep `/tmp/e2e_server.log` for patterns | Code path verification (fallback triggered?) |
| **API** | Check response content/status | Functional correctness |
| **State** | Query DB / check filesystem | Side effects landed? |
| **Process** | Check port/PID/files | Service behavior |

**Selection by scenario type:**
- **Positive**: API + State
- **Negative**: **Log REQUIRED** + API
- **Boundary**: Log + API

**The rule that matters most:** For negative scenarios, "API returned success" alone is NOT proof. Log probe must confirm the error/fallback path was actually taken. This is the single most common verification failure.

## Step 6: Execute

Three-phase protocol per test card. **Phase C always executes.**

**Phase A — Backup**: Copy each file to `{name}.e2e.bak`. Verify backup exists and matches.

**Phase B — Manipulate + Test + Collect**:
1. Modify environment per test card
2. Restart service if needed (keep elevated log level)
3. Execute trigger
4. **Collect ALL probe evidence NOW** (before Phase C)

**Phase C — Restore** (mandatory, even on failure):
1. Restore `.e2e.bak` → original path
2. Verify checksum match **before** deleting backups
3. Delete `.e2e.bak` files
4. Restore original log level; restart if Phase B restarted
5. **On restore failure**: STOP. Print `cp <bak> <original>` commands for each file. Do not continue.

**Manipulation methods**: modify config files, set env vars, send special inputs, change endpoints to unreachable addresses.

**Safety**: never modify DB files, never delete files, always restore.

## Step 7: Report

```
=== E2E Verification Report ===
Project: {name} | Build: {type} ({duration}) | Server: :{port}
Intent: {sources} (commits {range})

| # | Type | Target | Result | Evidence |
|---|------|--------|--------|----------|
| 1 | positive | ... | PASS | API: completed=true |
| 2 | negative | ... | PASS | Log: "fallback to X" (L847) + API: ok |
| 3 | boundary | ... | FAIL | Expected: error msg / Actual: panic |

Restore: {status} | Summary: {pass}/{total} PASS
```

On FAIL: cite log lines, show expected vs actual, suggest causes.

## Red Flags

| Trap | Reality |
|------|---------|
| "API success = fallback worked" | No log probe = unverified |
| "No error handling in diff" | `?`, `unwrap_or`, `else`, `match Err` count |
| "Config too complex to modify" | That's the most important scenario to test |
| "Config restored, should be fine" | Verify checksums |
| "Unit tests cover this" | Unit ≠ E2E |
| "User didn't ask to test this" | Git diff did |
