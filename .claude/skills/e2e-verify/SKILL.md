---
name: e2e-verify
description: >
  E2E verification with intent-aware scenario design and multi-probe evidence
  chains. Supports TWO modes: (A) live server testing with config manipulation,
  (B) integration test authoring for library/core modules. Auto-detects mode
  from project structure. Use when: (1) user says "verify", "e2e test",
  "production verify", "validate module", "生产验证", "验证模块", (2) after
  completing a major feature that adds endpoints, tools, or code paths, (3) user
  runs /e2e-verify [target]. Analyzes git diff + user instructions to design
  positive/negative/boundary scenarios.
  Flags: --debug (verbose output), --skip-build (reuse last binary).
---

# E2E Verify

Nine steps. No skipping.

| # | Step | What happens |
|---|------|-------------|
| 0 | Intent Analysis | User instructions + git diff + commit messages → test target list |
| 1 | Mode Selection | Detect: **Server Mode** (live service) or **Library Mode** (integration tests) |
| 2 | Coverage Gap Analysis | Existing tests vs diff paths → gap list |
| 3 | Infrastructure | Server Mode: Kill → Build → Start → Verify. Library Mode: Build → Run existing tests |
| 4 | Scenario Design | Test cards: positive + negative + boundary per target |
| 5 | Execute | Server: backup → manipulate → probe → restore. Library: write tests → run → verify |
| 6 | Report | Evidence-based summary table |

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

## Step 1: Mode Selection

Determine execution mode based on project structure and test target nature.

**Decision criteria:**

| Signal | Mode |
|--------|------|
| Changed files are in `src/` library code with no server endpoint | **Library Mode** |
| Changed files are HTTP handlers / RPC methods / server config | **Server Mode** |
| Project has `Cargo.toml` with `[[bin]]` and changes are in binary crate | **Server Mode** |
| Project has `Cargo.toml` with `[lib]` and changes are in library modules | **Library Mode** |
| Changes span both library and server code | **Both** (library tests first, then server verification) |
| `scripts/` directory has E2E client scripts | Suggests **Server Mode** available |

**Library Mode** means: write and run integration tests (`cargo test`, `pytest`, `go test`, `jest`).
**Server Mode** means: start a live server, manipulate its environment, hit it with probes.

```
Mode: Library Mode
Reason: All changes are in core/src/agent_loop/ (library crate, no server endpoints)
Test runner: cargo test -p alephcore --lib
```

## Step 2: Coverage Gap Analysis

**Before designing scenarios, analyze what's already covered.**

1. **List existing tests** for changed modules:
   ```bash
   # Rust
   grep -n "fn test_\|async fn test_" <changed_files> <test_files>
   # Python
   grep -n "def test_\|class Test" tests/
   # JS/TS
   grep -rn "it(\|test(\|describe(" __tests__/
   ```

2. **Map diff paths to tests**: For each function/branch added in the diff, check if an existing test exercises that path.

3. **Output gap table:**
   ```
   | Code Path | Existing Test | Gap? |
   |-----------|--------------|------|
   | stop_hooks block flow | test_hook_block (unit only) | YES — no integration test with main loop |
   | tool_refresh mid-loop | test_tool_refresh_signal (unit) | YES — no test verifying registry rebuild in loop |
   | chain_context depth limit | test_child_returns_none | NO — covered |
   ```

4. **Prioritize gaps** by risk: error handling paths > happy paths > edge cases.

**This step prevents two failure modes:**
- Writing redundant tests for already-covered paths
- Missing critical gaps because "the module has tests" (but not for the new code)

## Step 3: Infrastructure

### Server Mode

Discover project specifics from CLAUDE.md, README, Makefile/justfile/package.json. Check `scripts/` and `references/` in this skill's directory for existing tools.

**Critical details only:**
- **Kill**: Read CLAUDE.md for process management rules before killing anything.
- **Build**: Compile the project. Skip with `--skip-build` flag.
- **Start**: Start with elevated log level (`RUST_LOG=debug` / `LOG_LEVEL=DEBUG` / equivalent). Redirect stdout+stderr to `/tmp/e2e_server.log`. Record original level for restore.
- **Verify**: If `scripts/` has a client library, use it. Otherwise: `kill -0 $PID` + port check + optional auth.

### Library Mode

- **Build**: Compile and run existing tests for changed modules to establish baseline.
  ```bash
  cargo test -p <crate> --lib -- <module_path> 2>&1 | tail -20
  ```
- **Baseline**: Record pass count. All existing tests must pass before writing new ones.
- **No Kill/Start/Verify needed** — the test runner is the infrastructure.

## Step 4: Scenario Design

For each target from Step 0 that has a gap from Step 2, produce a **test card**.

### Server Mode test card:

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

### Library Mode test card:

```
Target: [negative] Stop hook blocks → loop continues with injected message
Setup:
  - Mock: ProbeProvider returns [tool_call, completion, completion]
  - Hook: Shell script that blocks first time (exit 2), allows second (exit 0)
Trigger: agent.run("test blocking hook", &mut cb)
Assertions:
  - result.iterations >= 3 (tool + block + retry)
  - result.final_text contains expected response
  - Hook marker file cleaned up
Isolation: Temp files use unique paths to avoid test interference
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

### Server Mode Probes

| Probe | Method | Best for |
|-------|--------|----------|
| **Log** | grep `/tmp/e2e_server.log` for patterns | Code path verification (fallback triggered?) |
| **API** | Check response content/status | Functional correctness |
| **State** | Query DB / check filesystem | Side effects landed? |
| **Process** | Check port/PID/files | Service behavior |

### Library Mode Probes

| Probe | Method | Best for |
|-------|--------|----------|
| **Assertion** | `assert_eq!`, `assert!`, `expect` in test code | Functional correctness |
| **Capture** | Record provider calls, callback invocations, messages | Code path verification |
| **Timing** | `Instant::now()` + duration checks | Concurrency / timeout behavior |
| **Structure** | Inspect message history, registry state after run | Side effects landed? |

**Selection by scenario type:**

| Type | Server Mode | Library Mode |
|------|-------------|-------------|
| Positive | API + State | Assertion + Structure |
| Negative | **Log REQUIRED** + API | **Capture REQUIRED** + Assertion |
| Boundary | Log + API | Capture + Assertion + Timing |

**The rule that matters most:** For negative scenarios, "returned success" alone is NOT proof.
- Server Mode: Log probe must confirm the error/fallback path was actually taken.
- Library Mode: Captured data (provider calls, callback events, message history) must show the error path was exercised. A passing assertion on the final result is insufficient without verifying the intermediate path.

## Step 5: Execute

### Server Mode — Three-phase protocol

Per test card. **Phase C always executes.**

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

### Library Mode — Write-Run-Verify protocol

**Phase A — Write tests**:
1. Add test functions to the appropriate test module (prefer existing integration test files)
2. Construct mock/probe infrastructure that captures intermediate state
3. Set up test conditions through mock configuration (not config file manipulation)
4. Include assertions for BOTH final result AND intermediate path verification

**Phase B — Run and collect**:
1. Run only the new tests first to verify they compile and pass
2. Run the full module test suite to verify no regressions
3. Record pass counts (before vs after)

**Phase C — Cleanup**:
1. Remove any temp files created during tests (markers, lock files)
2. Verify no test pollution (tests must be isolated and idempotent)
3. If a test uses external resources (temp files, ports), use unique identifiers

**Test isolation rules:**
- Each test must be independently runnable (`cargo test <test_name>`)
- No shared mutable state between tests (no static mut, no shared temp files without unique names)
- Async tests must handle cancellation cleanly (no leaked tasks)
- Temp file paths should include test name or random suffix to prevent collisions

## Step 6: Report

```
=== E2E Verification Report ===
Project: {name} | Mode: {Server|Library} | Build: {type} ({duration})
Intent: {sources} (commits {range})

| # | Type | Target | Result | Evidence |
|---|------|--------|--------|----------|
| 1 | positive | ... | PASS | API: completed=true |
| 2 | negative | ... | PASS | Log: "fallback to X" (L847) + API: ok |
| 3 | boundary | ... | FAIL | Expected: error msg / Actual: panic |

Tests: {before_count} → {after_count} (+{new_count} new)
Restore: {status} | Summary: {pass}/{total} PASS
```

On FAIL: cite log lines or assertion output, show expected vs actual, suggest causes.

### Library Mode report additions:
```
Coverage Delta:
  Before: {N} tests in {module}
  After: {N+M} tests (+{M} new integration probes)
  New tests:
    - test_stop_hook_allows_completion (E11, positive)
    - test_stop_hook_blocks_then_allows (E12, negative)
    ...
```

## Red Flags

| Trap | Reality |
|------|---------|
| "API success = fallback worked" | No log/capture probe = unverified |
| "No error handling in diff" | `?`, `unwrap_or`, `else`, `match Err` count |
| "Config too complex to modify" | That's the most important scenario to test |
| "Config restored, should be fine" | Verify checksums |
| "Unit tests cover this" | Unit ≠ E2E integration |
| "User didn't ask to test this" | Git diff did |
| "The module has tests already" | Check the GAP ANALYSIS — new code may be uncovered |
| "I'll just test the happy path" | Diff added error handling → negative scenario is mandatory |
| "Library code doesn't need E2E" | Integration probes through the full call chain ARE E2E for libraries |
| "Mock returns success so it works" | Captured intermediate state must verify the PATH, not just the outcome |
