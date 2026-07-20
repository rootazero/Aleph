# E2E Verify Skill Redesign

**Date**: 2026-03-29
**Status**: Draft
**Scope**: Rewrite `aleph-e2e-verify` skill → generic `e2e-verify` skill

## Problem

The current `aleph-e2e-verify` skill has two critical flaws:

1. **No change intent analysis** — it tests by module name, not by what actually changed. It doesn't read git diff or user instructions to understand *what* needs testing.
2. **No negative/boundary scenarios** — it only verifies "happy path works." It never constructs failure conditions to test error handling, fallback, retry, or degradation paths.

Real example: after implementing provider fallback, the skill tested "provider responds normally" and reported PASS — without ever verifying that fallback actually triggers when the primary provider fails.

## Design Decisions

- **Name**: `e2e-verify` (drop `aleph-` prefix, project-agnostic)
- **Config mutation**: allowed with mandatory backup+restore, no user confirmation needed
- **Intent sources**: user instructions + git diff, cross-referenced
- **Probes**: multi-probe (log + API + state + process), with auto log-level elevation
- **Structure**: single SKILL.md with full methodology; `references/` for project-specific materials

## New Flow: 8 Steps

| # | Step | Core Action | Change from Old |
|---|------|-------------|-----------------|
| 0 | **Intent Analysis** | Read user instructions + git diff → extract test target list | **NEW** |
| 1 | Kill | Kill old processes | Unchanged |
| 2 | Build | Compile | Unchanged |
| 3 | Start | Start service with elevated log level | **Improved**: auto `RUST_LOG=debug` or equivalent |
| 4 | Verify | Confirm service ready | Unchanged |
| 5 | **Scenario Design** | For each target: positive + negative + boundary test cards | **Rewritten** |
| 6 | **Execute** | Backup config → manipulate environment → run tests → restore config | **Rewritten**: environment manipulation protocol |
| 7 | Report | Summary table with probe evidence chain | **Improved**: evidence-based |

## Step 0: Intent Analysis

Three information sources, cross-referenced to produce test target list:

**Source 1: User Instructions**
- Parse explicit target: `/e2e-verify provider fallback` → target is "provider fallback"
- If no arguments: rely entirely on sources 2 and 3

**Source 2: Git Diff Analysis**
- `git diff HEAD~N..HEAD` — N determination strategy:
  - Default: N=1 (last commit)
  - If diff is empty, expand to last non-empty diff commit via `git log --oneline` scan
  - If still empty (no recent changes), rely entirely on Source 1 (user instructions) and prompt user to specify test scope
- Extract from diff:
  - New/modified functions and modules → test targets
  - New error handling paths → require negative scenarios
  - New config options / conditional branches → require boundary scenarios
  - New log/tracing statements → available probes

**Source 3: Related Documentation and Tests**
- Read commit messages to understand change intent
- Check for related unit/integration tests to understand covered vs uncovered paths

**Output — Test Target List**:

```
Test Targets:
1. [positive] Provider normal response completes chat — source: user + diff
2. [negative] Auto fallback when default provider unavailable — source: diff (new fallback logic)
3. [boundary] Graceful error when all providers unavailable — source: diff (error handling path)
```

Each target tagged `[positive]`/`[negative]`/`[boundary]` with source attribution. **Negative scenarios are NOT optional** — if diff contains error handling/fallback/degradation logic, corresponding negative scenarios are mandatory.

## Step 5: Scenario Design — Test Cards

Each test target must produce a complete test card:

```
Target: [negative] Auto fallback when default provider unavailable
Preconditions:
  - Environment manipulation: Change default provider API key to invalid value
  - Backup: ~/.aleph/config.toml → ~/.aleph/config.toml.e2e.bak
Trigger:
  - chat("Summarize today's weather")  # any request requiring LLM
Expected Behavior:
  - Log probe: "fallback" or "retry" related tracing appears
  - API probe: chat returns completed=true (not error)
  - Response probe: returned text is non-empty, tool_calls normal
Restore:
  - Restore config.toml.e2e.bak → config.toml
Judgment:
  - PASS: all probes satisfied
  - FAIL: any probe fails, output actual value as evidence
```

**Mandatory scenario rules**:

| Type | When Required | Example |
|------|--------------|---------|
| Positive | At least one per test target | Provider normal → chat completes |
| Negative | When diff has error handling / fallback / degradation code | Default provider fails → fallback triggers |
| Boundary | When diff has conditional branches / thresholds / null handling | All providers unavailable → graceful error |

**Enforcement**: If diff contains error-handling patterns in newly added logic, corresponding negative/boundary scenarios are mandatory. Language-specific patterns to watch for:
- **Rust**: `fallback`, `retry`, `?`, `unwrap_or`, `match Err`, `Option::None`, `else`
- **Python**: `except`, `try/finally`, `None`, `raise`, `fallback`
- **Go**: `if err != nil`, `default:`, `fallback`
- **JS/TS**: `.catch()`, `try/catch`, `null`, `undefined`, `??`, `fallback`
- **General**: any word matching `fallback`, `retry`, `timeout`, `degrade`, `error`, `recover`

**When no negative scenarios exist**: If diff genuinely contains no error-handling paths, only positive scenarios are required. Do NOT fabricate negative scenarios for completeness — test what actually changed.

## Step 6: Environment Manipulation Protocol

Three-phase protocol for constructing failure conditions:

**Phase A: Backup**
1. Identify files to modify
2. Copy each to `{filename}.e2e.bak`
3. Record all backup paths (for Phase C restore)
4. Verify backup files exist and content matches

**Phase B: Manipulate + Test + Collect**
1. Modify environment per test card preconditions
2. If service restart needed, restart with the same debug log environment variable from Step 3 (e.g., `RUST_LOG=debug ./start-command &`)
3. Wait for service ready
4. Execute test card trigger actions
5. **Collect all probe results NOW** — read log files, capture API responses, query state BEFORE Phase C
6. Store probe evidence for Step 7 report

**Phase C: Restore (MUST execute regardless of test success/failure)**
1. Restore all `.e2e.bak` files to original paths
2. Verify restored files match backups (compare checksums before deleting backups)
3. Delete `.e2e.bak` files
4. Restore log level environment variable to original value
5. If Phase B restarted service, restart again with original environment (no debug log override)
6. **On restore failure**: STOP immediately, print manual recovery commands (e.g., `cp ~/.aleph/config.toml.e2e.bak ~/.aleph/config.toml`), list all remaining `.e2e.bak` files on disk, do not continue subsequent tests

**Manipulation methods**:

| Method | Use Case | Example |
|--------|----------|---------|
| Modify config file | Test config-related fallback/degradation | Change API key to invalid value |
| Set environment variable | Test environment-aware behavior | `ALEPH_PROVIDER_TIMEOUT=1ms` |
| Send special input | Test input boundary handling | Overlong text, empty message, special chars |
| Simulate external failure | Test network/service dependency | Change endpoint to unreachable address |

**Safety redlines**:
- NEVER modify database files (corruption risk)
- NEVER delete any file (only modify content)
- Restore failure handling is defined in Phase C step 6 above

## Probe System

**Auto log-level elevation**: Step 3 starts service with `RUST_LOG=debug` (or project-equivalent log env var). Restore original log level after testing.

**Four probe types**:

| Probe | Collection Method | Best For |
|-------|------------------|----------|
| **Log** | Read service stdout/log file, pattern match | Verify code path executed (fallback triggered, retry attempted) |
| **API** | Check API/WebSocket response content and status | Verify functional result correct (success, non-empty content) |
| **State** | Query database/filesystem state changes | Verify side effects landed (data written, files generated) |
| **Process** | Check port/process/file changes | Verify service behavior (port listening, process alive) |

**Probe selection rules**:
- Positive: API probe (result correct) + State probe (side effects landed)
- Negative: **Log probe REQUIRED** (prove error handling path was taken) + API probe (final result still correct or graceful error)
- Boundary: Log probe + API probe (confirm no panic, no hang)

**Critical constraint**: For negative scenarios, API probe showing "result correct" is INSUFFICIENT alone — log probe must prove the fallback/error path was actually taken, not that the primary path happened to recover.

**Log capture method** (generic):
- Service started with `&`, stdout/stderr redirected to temp file
- After test, read log file, grep/pattern match for key evidence
- Report cites specific log lines as evidence

## Project Adaptation

**Generic parts in SKILL.md** (applicable to any project):
1. Intent analysis methodology (Step 0)
2. Three-layer scenario model (positive/negative/boundary)
3. Test card format
4. Environment manipulation protocol (backup-manipulate-restore)
5. Probe selection decision tree
6. Log level elevation strategy
7. Report format

**Project-specific discovery** (Claude figures out by exploring):

| Information | Discovery Method |
|-------------|-----------------|
| Service start command | Read Makefile / justfile / package.json / README |
| Service port | Read config or startup logs |
| Log level env var | `RUST_LOG` / `LOG_LEVEL` / `DEBUG` etc., infer by language stack |
| Authentication | Read project code or config |
| Database location and schema | Read DB connection config in code |
| Available API/RPC endpoints | Read route registration code |

**references/ folder**: stores project-specific reference materials. For Aleph project, retains:
- `references/websocket-protocol.md` — Aleph WS protocol
- `scripts/aleph_e2e_client.py` — Aleph Python client

**Integration with flow steps** (when project-specific files exist):
- **Step 4 (Verify)**: If `scripts/` contains a client library, use it for server readiness checks
- **Step 5 (Scenario Design)**: If `references/` contains protocol docs, use them to determine correct trigger syntax (e.g., WebSocket message format, API call structure)
- **Step 6 (Execute)**: If `scripts/` contains a test client, import it instead of writing from scratch

When used on a project without these files, Claude explores the project's test infrastructure and writes its own.

**Project discovery guidance in SKILL.md**:
```
Project Exploration (first use on a project):
1. Read README / Makefile / justfile → build and start commands
2. Read CLAUDE.md → project constraints and process management rules
3. Check references/ directory → existing protocol docs or test clients
4. Check scripts/ directory → reusable test tools
5. If none found → write test scripts based on project language stack
```

## Report Format

```
=== E2E Verification Report ===
Project: {name} | Build: {type} ({duration}) | Server: :{port}
Log Level: debug | Config Backups: {count} files

Intent Sources: user instructions + git diff (commits abc123..def456)

| # | Type | Test Target | Result | Evidence |
|---|------|------------|--------|----------|
| 1 | positive | Provider normal response | PASS | API: completed=true, text.len=342 |
| 2 | negative | Fallback triggers | PASS | Log: "fallback to provider X" (line 847) + API: completed=true |
| 3 | boundary | All providers unavailable | PASS | Log: "all providers exhausted" + API: error="no available provider" |

Environment Restore: ✓ All configs restored (2 files)
Summary: 3/3 PASS
```

On FAIL: cite actual log lines, show expected vs actual values, suggest possible causes.

## Red Flags

| Thinking Trap | Correct Action |
|--------------|----------------|
| "API returned success, fallback must have worked" | No log probe proving fallback path = unverified |
| "No error handling code in diff, no negative scenarios needed" | Look again — `?`, `unwrap_or`, `else`, `match Err` all count |
| "Config too complex to modify, skip this negative scenario" | This is exactly the scenario most worth testing. Find minimum change point |
| "Restored config, should be fine" | Must verify restored file content matches backup |
| "Unit tests already cover this path" | Unit test ≠ E2E. Integration environment may behave differently |
| "User didn't ask to test this" | Git diff did. Targets from change analysis are equally mandatory |

## File Structure After Redesign

```
.claude/skills/e2e-verify/
├── SKILL.md                          # Generic methodology (rewritten)
├── references/
│   └── websocket-protocol.md         # Aleph-specific WS protocol (retained)
└── scripts/
    └── aleph_e2e_client.py           # Aleph-specific Python client (retained)
```
