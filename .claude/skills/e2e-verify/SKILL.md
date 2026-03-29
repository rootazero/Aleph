---
name: e2e-verify
description: |
  End-to-end verification skill. Analyzes change intent from git diff and user instructions,
  designs positive/negative/boundary test scenarios, manipulates environment to construct failure
  conditions, and verifies with multi-probe evidence chains.
  Triggers: "verify", "e2e test", "e2e verify", "production verify", "validate module",
  "生产验证", "验证模块", and after completing a major feature.
  Flags: --debug (verbose probe output), --skip-build (skip compilation step)
---

# E2E Verify Skill

## Overview

| Step | Name | Action |
|------|------|--------|
| 0 | Intent Analysis | Read user instructions + git diff + related docs -> test target list |
| 1 | Kill | Find and kill old server processes |
| 2 | Build | Compile / build the project |
| 3 | Start | Start service with elevated log level, capture stdout/stderr |
| 4 | Verify | Confirm service is alive, port listening, auth working |
| 5 | Scenario Design | Produce test cards: positive + negative + boundary per target |
| 6 | Execute | Backup -> Manipulate + Test + Collect probes -> Restore |
| 7 | Report | Evidence-based summary table |

---

## Step 0: Intent Analysis

Cross-reference three sources to produce a test target list.

**Source 1: User Instructions**
- Parse explicit target from user message (e.g., `/e2e-verify provider fallback`)
- If no arguments: rely on Sources 2 and 3

**Source 2: Git Diff Analysis**
- Run `git diff HEAD~N..HEAD` with this N determination strategy:
  - Default: N=1 (last commit)
  - If diff is empty, scan `git log --oneline` and expand to last non-empty diff commit
  - If still empty (no recent changes), rely on Source 1 and prompt user to specify scope
- Extract from diff:
  - New/modified functions and modules -> test targets
  - New error handling paths -> **require negative scenarios**
  - New config options / conditional branches -> **require boundary scenarios**
  - New log/tracing statements -> available probes

**Source 3: Related Documentation and Tests**
- Read commit messages to understand change intent
- Check for related unit/integration tests to find covered vs uncovered paths

**Output — Test Target List:**

```
Test Targets:
1. [positive] <description> — source: <user | diff | both>
2. [negative] <description> — source: <diff (new error handling logic)>
3. [boundary] <description> — source: <diff (conditional branch)>
```

Each target tagged `[positive]`/`[negative]`/`[boundary]` with source attribution. **Negative scenarios are NOT optional** — if diff contains error handling / fallback / degradation logic, corresponding negative scenarios are mandatory.

---

## Step 1: Kill Old Processes

1. Read CLAUDE.md for project-specific process management rules (kill order, wait times, safety checks)
2. Discover the server process name from build config or CLAUDE.md
3. Find and kill all running server instances
4. Wait for processes to terminate (respect any documented wait times)
5. Verify no server processes remain

---

## Step 2: Build

1. Discover the build command by reading Makefile / justfile / package.json / Cargo.toml / README
2. If `--skip-build` flag is set, skip this step
3. Run the build command (prefer release build for E2E accuracy)
4. Record build duration for the report

---

## Step 3: Start Service

1. Determine the project's log-level environment variable by language stack:
   - Rust: `RUST_LOG=debug`
   - Python: `LOG_LEVEL=DEBUG`
   - Node.js: `DEBUG=*` or `LOG_LEVEL=debug`
   - Go: infer from code
2. Start the service in background with elevated log level, redirecting stdout+stderr to a temp file:
   ```
   <LOG_ENV>=debug <start_command> > /tmp/e2e_server.log 2>&1 &
   ```
3. Record the original log level (if any) for restore in Step 6 Phase C
4. Record the PID for later management

---

## Step 4: Verify Service Ready

1. Check `references/` and `scripts/` directories for existing readiness-check tools or client libraries
2. If found, use them. If not, verify manually:
   - Process is alive (`kill -0 $PID`)
   - Port is listening (discover port from config, startup logs, or code)
   - Optional: auth check if project uses authentication (discover auth method from code/config)
3. Retry with backoff (up to 30s) until ready or timeout

---

## Step 5: Scenario Design

For each test target, produce a test card:

```
Target: [type] <description>
Preconditions:
  - Environment manipulation: <what to change>
  - Backup: <source_path> -> <source_path>.e2e.bak
Trigger:
  - <action to execute>
Expected Behavior:
  - Log probe: <pattern to match in logs>
  - API probe: <expected response properties>
  - State probe: <expected side effects>
Restore:
  - <restore instructions>
Judgment:
  - PASS: all probes satisfied
  - FAIL: any probe fails, output actual value as evidence
```

**Mandatory scenario rules:**

| Type | When Required | Example |
|------|--------------|---------|
| Positive | At least one per test target | Normal request completes successfully |
| Negative | When diff has error handling / fallback / degradation code | Primary fails -> fallback triggers |
| Boundary | When diff has conditional branches / thresholds / null handling | All options exhausted -> graceful error |

**Enforcement** — Language-specific error-handling keywords that mandate negative/boundary scenarios:

- **Rust**: `fallback`, `retry`, `?`, `unwrap_or`, `match Err`, `Option::None`, `else`
- **Python**: `except`, `try/finally`, `None`, `raise`, `fallback`
- **Go**: `if err != nil`, `default:`, `fallback`
- **JS/TS**: `.catch()`, `try/catch`, `null`, `undefined`, `??`, `fallback`
- **General**: any word matching `fallback`, `retry`, `timeout`, `degrade`, `error`, `recover`

**When no negative scenarios exist**: If diff genuinely contains no error-handling paths, only positive scenarios are required. Do NOT fabricate negative scenarios for completeness — test what actually changed.

---

## Probe System

**Four probe types:**

| Probe | Collection Method | Best For |
|-------|------------------|----------|
| **Log** | Read service stdout/log file, pattern match | Verify code path executed (fallback triggered, retry attempted) |
| **API** | Check API/WebSocket response content and status | Verify functional result correct |
| **State** | Query database/filesystem state changes | Verify side effects landed |
| **Process** | Check port/process/file changes | Verify service behavior |

**Selection rules per scenario type:**

- **Positive**: API probe (result correct) + State probe (side effects landed)
- **Negative**: **Log probe REQUIRED** (prove error path taken) + API probe (result still correct or graceful error)
- **Boundary**: Log probe + API probe (confirm no panic, no hang)

**Critical constraint**: For negative scenarios, API probe showing "result correct" is INSUFFICIENT alone. Log probe must prove the fallback/error path was actually taken, not that the primary path happened to succeed.

---

## Step 6: Execute

Three-phase protocol per test card.

### Phase A: Backup

1. Identify files to modify per test card preconditions
2. Copy each to `{filename}.e2e.bak`
3. Record all backup paths for Phase C
4. Verify backup files exist and content matches originals

### Phase B: Manipulate + Test + Collect

1. Modify environment per test card preconditions
2. If service restart needed, restart with the same elevated log level from Step 3
3. Wait for service ready (reuse Step 4 verification)
4. Execute test card trigger actions
5. **Collect all probe results NOW** — read log file, capture API responses, query state BEFORE Phase C
6. Store probe evidence for Step 7 report

### Phase C: Restore (MUST execute regardless of test success/failure)

1. Restore all `.e2e.bak` files to original paths
2. Verify restored file checksums match backups BEFORE deleting backups
3. Delete `.e2e.bak` files
4. Restore log level environment variable to original value
5. If Phase B restarted the service, restart again with original environment (no debug override)
6. **On restore failure**: STOP immediately. Print manual recovery commands listing each `.e2e.bak` file and its restore target. Do not continue subsequent tests.

**Manipulation methods:**

| Method | Use Case | Example |
|--------|----------|---------|
| Modify config file | Test config-related fallback/degradation | Change API key to invalid value |
| Set environment variable | Test environment-aware behavior | Set timeout to 1ms |
| Send special input | Test input boundary handling | Overlong text, empty message, special chars |
| Simulate external failure | Test network/service dependency | Change endpoint to unreachable address |

**Safety redlines:**

- NEVER modify database files (corruption risk)
- NEVER delete any file (only modify content)
- ALWAYS execute Phase C even if Phase B fails or panics

---

## Step 7: Report

```
=== E2E Verification Report ===
Project: {name} | Build: {type} ({duration}) | Server: :{port}
Log Level: debug | Config Backups: {count} files

Intent Sources: {user instructions | git diff | both} (commits {range})

| # | Type | Test Target | Result | Evidence |
|---|------|------------|--------|----------|
| 1 | positive | ... | PASS | API: ... |
| 2 | negative | ... | PASS | Log: "..." (line N) + API: ... |
| 3 | boundary | ... | FAIL | Expected: ... / Actual: ... |

Environment Restore: {status}
Summary: {pass_count}/{total} PASS
```

**On FAIL**: Cite actual log lines, show expected vs actual values, suggest possible causes based on the evidence.

---

## Red Flags — Thinking Traps

| Thinking Trap | Correct Action |
|--------------|----------------|
| "API returned success, fallback must have worked" | No log probe proving fallback path = unverified |
| "No error handling code in diff, skip negative scenarios" | Look again — `?`, `unwrap_or`, `else`, `match Err` all count |
| "Config too complex to modify, skip this negative scenario" | This is exactly the scenario most worth testing. Find minimum change point |
| "Restored config, should be fine" | Must verify restored file content matches backup checksums |
| "Unit tests already cover this path" | Unit test != E2E. Integration environment may behave differently |
| "User didn't ask to test this" | Git diff did. Targets from change analysis are equally mandatory |

---

## Project Discovery

When first using this skill on a project, explore:

1. Read README / Makefile / justfile / package.json -> build and start commands
2. Read CLAUDE.md -> project constraints, process management rules, known ports
3. Check `references/` directory in skill folder -> existing protocol docs or API specs
4. Check `scripts/` directory in skill folder -> reusable test clients or helpers
5. If none found -> write test scripts based on the project's language stack

**Integration with flow steps when project-specific files exist:**

- **Step 4 (Verify)**: If `scripts/` contains a client library, use it for readiness checks
- **Step 5 (Scenario Design)**: If `references/` contains protocol docs, use them for correct trigger syntax (WebSocket message format, API structure, etc.)
- **Step 6 (Execute)**: If `scripts/` contains a test client, import/use it instead of writing from scratch
