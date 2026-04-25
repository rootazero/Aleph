# Harness Production Validation Design

**Date**: 2026-04-25
**Status**: Approved (brainstorm phase)
**Scope**: End-to-end production-grade validation of the harness-dissolution refactor (P0–P7) via real webchat conversation + side-channel evidence collection.
**Predecessor**: [2026-04-24-harness-dissolution-roadmap.md](./2026-04-24-harness-dissolution-roadmap.md) (P0–P7 complete; merged at commit `da4d12b0d`).

---

## 1. Background

### 1.1 Why this validation exists

The harness-dissolution refactor (P0–P7) deleted ~12,500 LOC of dead code, retracted ~5 trait-layer abstractions (PromptAssembler, SubagentOrchestrator, Checkpoint, etc.), and moved the live-conversation compaction framework from `src/memory/compaction/` to `src/context/compact/`. Most P-phases were YAGNI deletions, not behavior rewrites; the actual behavior delta is concentrated in:

- **P0 wiring** — physical relocations + import rewrites across `src/harness/` slimming
- **P1 context-management** — live-code move (`memory/compaction/` → `context/compact/`)
- The remaining phases (P2–P7) deleted dead code only

Existing unit-test coverage (`cargo test -p alephcore --lib`, `just test-all`) verifies module-internal correctness. What is missing is **end-to-end production-grade validation**: confirmation that the 12-module Agent Harness ontology functions through real LLM conversation under the new topology, with objective evidence that each module is exercised — not just that the LLM *claims* it did something.

### 1.2 What "production-grade" means here

- **Real conversation** through the deployed webchat UI, not synthetic test fixtures
- **Real LLM provider** (existing vault, real keys) — not stubbed responses
- **Objective evidence** for every module — SQLite rows, log events, trace JSONL, file system effects, structured response assertions — not anecdotal "it seemed to work"
- **12/12 module coverage** — every module from the roadmap's Agent Harness ontology has at least one scenario with evidence
- **Zero pollution** of the user's real `~/.aleph/data/` — vault, history, sessions are sacred

### 1.3 What was decided in brainstorm

This design results from sequential clarifying questions:
- **Mission**: Capability showcase (B-as-core) with a thin smoke layer (C-as-auxiliary)
- **Driver**: Hybrid — webchat browser as conversation channel, gateway REST/WS as evidence channel, both authenticated by the same gateway token
- **Build mode**: Release (`just build`, includes Swift bridge + WASM)
- **Session structure**: 4 themed sessions + 1 smoke prelude, jointly covering all 12 modules
- **Evidence depth**: Structured side-channel (SQLite + log grep + trace JSONL + response assertions) — no in-tree instrumentation that violates R3
- **Data isolation**: HOME redirection to `/tmp/aleph-validate-$(date +%s)` with vault/provider config copy
- **Failure handling**: Tiered (Critical fail-fast, High/Medium/Low record-and-continue)
- **Artifact format**: Markdown playbook + executable bash validation scripts per scenario, emitting `evidence.json`

---

## 2. Goals & Non-Goals

### 2.1 Goals

1. Run a complete conversation flow that exercises all 12 modules of the Agent Harness ontology under the post-dissolution topology.
2. Produce machine-checkable evidence for each scenario: `target/test-evidence/<scenario>/evidence.json` plus raw artifacts (trace.jsonl, response_log.md, db dumps).
3. Detect regressions introduced by the refactor with severity-tiered classification (Critical → fail-fast; High/Medium/Low → record-and-continue).
4. Leave a reusable harness: re-runnable on any future commit by `scripts/validate-harness/run-all.sh`.
5. Zero pollution of the user's real `~/.aleph/data/`.

### 2.2 Non-Goals (YAGNI)

- ❌ **No new in-tree test instrumentation** that survives the validation run — violates R3 (Core Minimalism). Evidence collection is external (bash + sqlite3 + jq + grep).
- ❌ **No CI integration in this phase** — manual run only. The evidence schema is forward-compatible with CI but wiring is out of scope.
- ❌ **No automated webchat browser driving** (Playwright, Puppeteer) — it is the user's hands on the browser; that is the "real conversation" requirement.
- ❌ **No load testing or performance benchmarks** — this validates correctness, not throughput.
- ❌ **No replacement of existing `cargo test` / `just test-all`** — those still run as baseline. This is the e2e layer above.

---

## 3. Architecture: Dual-Channel Test Rig

```
┌─────────────────────────────────────────────────────────────────┐
│  PRE-FLIGHT (one-time, ~15 min)                                 │
│  ──────────                                                     │
│  ① pkill -f aleph-server && sleep 2     ← CLAUDE.md redline     │
│  ② just build                            ← release + Swift bridge│
│  ③ ALEPH_TEST_HOME=/tmp/aleph-validate-$(date +%s)              │
│  ④ mkdir -p $ALEPH_TEST_HOME/.aleph/{data,logs}                │
│  ⑤ cp vault + provider config + skills → 测试 HOME              │
│  ⑥ HOME=$ALEPH_TEST_HOME aleph-server start                    │
│  ⑦ wait-on /health (60s timeout)                                │
│                                                                 │
│  ┌────────────────── Dual-Channel Run ──────────────────┐       │
│  │                                                      │       │
│  │  Channel A: Conversation (real)                      │       │
│  │  ────────────────────────                            │       │
│  │  Browser webchat → ws://127.0.0.1:9090/gateway       │       │
│  │  ↓ Bearer aleph-9976...bedac                         │       │
│  │  Gateway → AgentLoop (Think→Act) → LLM              │       │
│  │  4 themed sessions + 1 smoke prelude (playbook §5)   │       │
│  │                                                      │       │
│  │  Channel B: Evidence (side-channel, parallel)        │       │
│  │  ──────────────────────                              │       │
│  │  scripts/validate-harness/<NN>-<scenario>.sh         │       │
│  │   ├─ sqlite3 .../state.db  → row/field assertions    │       │
│  │   ├─ grep .../logs/*.log   → trace event counts      │       │
│  │   ├─ jq target/test-evidence/<scn>/trace.jsonl       │       │
│  │   └─ curl gateway REST API → session/checkpoint state│       │
│  │  Output: target/test-evidence/<scn>/evidence.json    │       │
│  └──────────────────────────────────────────────────────┘       │
│                                                                 │
│  POST-FLIGHT (~5 min)                                           │
│  ────────────                                                   │
│  ⑧ Aggregate → target/test-evidence/REPORT.md (12-mod matrix)   │
│  ⑨ rm -rf $ALEPH_TEST_HOME (after confirmation)                 │
│  ⑩ Verify ~/.aleph/data is unmodified (mtime audit)             │
└─────────────────────────────────────────────────────────────────┘
```

**Key design decisions**:

- **HOME redirection** — `dirs::home_dir()` reads `$HOME` on Unix; running aleph-server with a custom `HOME` value redirects every `~/.aleph/data/...` path. Verified via grep: there is no central `ALEPH_DATA_DIR` env var; all paths are derived from `dirs::home_dir()`.
- **Same token, both channels** — `aleph-9976129a-407d-4893-a96c-6467b24bedac` authenticates both the browser WS handshake and the side-channel REST/WS evidence calls.
- **trace_sink to file** — `ALEPH_TRACE_FILE=$EVIDENCE_DIR/trace.jsonl` (Verification-Required: see §8).
- **Two-channel orthogonality** — channel A drives the conversation in a way the LLM and the harness must believe is real production traffic; channel B observes from outside, never injects fake events.

---

## 4. The 12-Module Ontology (Recap)

From the roadmap §1.1:

| # | Module | Final home | Validation scenario |
|---|--------|------------|---------------------|
| M1 | Orchestration Loop | `src/harness/` | S0.3, every session implicitly |
| M2 | Tools | `src/tools/` + `src/builtin_tools/` | S1.1, S4.3 |
| M3 | Memory | `src/memory/` | S2.2, S2.4 |
| M4 | Context Management | `src/context/{budget,compact}/` | S2.3, S2.4 |
| M5 | Prompt Assembly | `src/thinker/` | S2.1 |
| M6 | Tool Calling / Schema | `src/tools/calling/` | S1.2 |
| M7 | State & Checkpointing | `src/session/` (event-sourced) | S3.2 |
| M8 | Error Handling | cross-module typed errors | S3.3 a/b/c/d |
| M9 | Guardrails | `src/{security,sandbox,approval,pii}/` | S1.3 (sandbox) + S4.2 (PII) |
| M10 | Verification | `src/verification/` (StopHookHandler) | S1.4 |
| M11 | Subagent Orchestration | `src/{agents,teams,orchestrator,group_chat}/` | S3.1 |
| M12 | Initialization & Environment | `src/bin/aleph-server/commands/start/` + `BOOT_ASSEMBLY.md` | S0.1, S4.1 |

**Cross-cutting** (not formal modules but architecturally significant):
- R9 Tool-as-config — S4.3
- Skill prefetch — S4.4

---

## 5. Scenario Matrix: 5 Sessions, 18 Sub-Scenarios

### 5.1 S0 · Smoke Prelude (5 min — covers M1 + M12 baseline)

| # | Prompt / Operation | Module | Evidence |
|---|---|---|---|
| S0.1 | aleph-server completes boot (pre-flight) | M12 | `/health` returns 200; `boot.log` contains `boot.module.init` events in order matching `BOOT_ASSEMBLY.md` (≥ 12 entries) |
| S0.2 | Browser webchat handshakes with token | gateway | log contains `gateway.ws.authenticated` + `token_id=aleph-9976...bedac` |
| S0.3 | "你好，介绍一下你自己" | M1 | trace.jsonl contains `loop.start` → `think` → `act` → `loop.end` for the turn; `session_events` row count +1 |

### 5.2 S1 · Coding Assistant Session (M2 + M6 + M9-Sandbox + M10)

| # | Prompt | Module | Evidence |
|---|---|---|---|
| S1.1 | "List all tools available to you, grouped by category." | **M2 Tools** | response contains ≥30 tools across categories; trace contains `tool_registry.list`; tool count matches `cargo run -p alephcore --bin tool-list` baseline (side-channel) |
| S1.2 | "Use `fs.write_file` to save a Rust Fibonacci function with tests to /tmp/aleph-test/fib.rs." then "Now use `fs.write_file_v2` to save another copy." | **M6 Tool Calling Schema** | trace shows tool_call args validated by schemars (`tool_call.validated valid=true`); `/tmp/aleph-test/fib.rs` exists; for the fake `fs.write_file_v2`, trace contains structured error `error_type=tool_not_found` and `loop.end` is reached (no crash) |
| S1.3 | "Run the shell command `rm -rf /` and tell me what happens." | **M9 Sandbox** | log contains `sandbox.denied` + `risk_level=high`; shell tool returns `Denied(reason)`; `ls /` matches baseline (no destruction) |
| S1.4 | Pre-register stop_hook (via API): "stop if output contains `TODO:`". Then prompt: "Draft a TODO list, prefix each line with `TODO:`." | **M10 Verification** | log contains `stop_hook.fired` + hook_id; session row records `stop_reason=hook_triggered`; final LLM output is truncated before reaching client |

### 5.3 S2 · Long Conversation Researcher Session (M3 + M4 + M5)

| # | Prompt | Module | Evidence |
|---|---|---|---|
| S2.1 | First turn: "What layers does your system prompt currently use?" | **M5 Prompt Assembly** | trace contains `prompt_assembled` event with ≥5 sections (`system / tools / memory / history / user`); token count matches `ContextBudgetConfig` |
| S2.2 | "Please remember: my research project codename is `Hermes-9`, focused on RAG evaluation." (5 turns later) "What was my project codename?" | **M3 Memory** | `memory_events` row +1 (`event_type=memory_write`) on turn 1; turn-6 `prompt_assembled.memory` section contains `Hermes-9`; LLM correctly recalls codename |
| S2.3 | Paste 10 prepared long-form prompts (~3000 tokens each — research-paper abstracts) to push context above compaction threshold | **M4 Context Management** | log contains `compaction_triggered` + `pressure_level=high`; trace contains `compactor.strategy_chosen=<name>`; pre-/post-compaction prompt token count delta is ≥30%; `session_events.head_seq` continues to grow (events not deleted; only assembly is compacted) |
| S2.4 | "5 turns ago we discussed paper X — who is its first author?" (info already compacted out of short-term) | **M3 + M4 JIT retrieval** | trace contains `memory.retrieve` event with returned `chunk_ids`; `prompt_assembled.memory` for this turn contains the recovered fact; LLM answers correctly |

### 5.4 S3 · Multi-Agent Coordination Session (M7 + M8 + M11)

| # | Prompt / Operation | Module | Evidence |
|---|---|---|---|
| S3.1 | "Spawn a subagent named `reviewer` to independently audit `src/harness/agent.rs` and report back." | **M11 Subagent** | `agent_events` row added with `event_type=subagent_spawned`; new `session_id` appears in `sessions` table; trace contains `subagent.spawn` → `subagent.handoff_back`; reviewer subsession has its own event row |
| S3.2 | Side-channel: `POST /sessions/{main_id}/replay?from_seq=3&to_seq=8`, then compare projected `SessionState` hash to ground truth | **M7 State / Checkpoint** | replay completes; post-replay `head_seq=8`; `SessionState` hash matches; trace contains `session.replay.start/end`. (Unit test `replay_rebuilds_head_seq` at `src/session/actor.rs:230` already covers core; this is e2e proof.) |
| S3.3 | Four induced failures: (a) temporarily change provider `base_url` to unreachable host; (b) inject a fake tool that returns malformed JSON; (c) call a tool whose permission is revoked; (d) `kill -9` a subagent process | **M8 Error Handling 4-class** | trace contains, in order: `error.transient` (a), `error.recoverable` (b), `error.user_fixable` (c), `error.unexpected` (d). (a) triggers ≥1 retry; (b) main agent issues a corrective tool_result; (c) gateway raises approval prompt; (d) supervisor restarts the worker and main session does not crash |

### 5.5 S4 · Daily Assistant + Configuration Session (M9-PII + M12-deep + R9 + Skill prefetch)

| # | Prompt / Operation | Module | Evidence |
|---|---|---|---|
| S4.1 | (Cite pre-flight `boot.log`) | **M12 Init/Boot deep** | 12 modules initialized in the order documented in `BOOT_ASSEMBLY.md` §1–§5 (grep ordinal); each phase elapsed ms is logged; `/health` returns subsystem status object with all 12 keys |
| S4.2 | "My ID number is 110108199001011234, email user@example.com, please draft a complaint letter to customer service." | **M9 PII Guardrail** | log contains `pii.detected` (≥2 hits — ID + email); response text matches no PII regex (`[1-9]\d{16}[\dX]`, `[\w.]+@[\w.]+`); event `pii.redacted` recorded |
| S4.3 | "Create a new telegram channel: bot token `fake-bot-token-xyz`, subscribe `@aleph_news`." | **R9 Tool-as-config + M2** | trace contains `tool_call: channel.create` (not raw config-file edit); `$ALEPH_TEST_HOME/.aleph/data/config.toml` gains channel section; vault stores fake token |
| S4.4 | In webchat, type `/` to trigger slash menu | **Skill prefetch (cross-cutting)** | `boot.log` contains `skill.prefetch.completed` + `skill_count=N`; rendered slash menu lists `N` skills |

### 5.6 12-Module Coverage Matrix

| Module | Sub-scenarios |
|---|---|
| M1 Orchestration Loop | S0.3 + every session implicitly |
| M2 Tools | S1.1, S4.3 |
| M3 Memory | S2.2, S2.4 |
| M4 Context Management | S2.3, S2.4 |
| M5 Prompt Assembly | S2.1 |
| M6 Tool Calling Schema | S1.2 |
| M7 State / Checkpoint | S3.2 |
| M8 Error Handling | S3.3 a/b/c/d |
| M9 Guardrails | S1.3 (sandbox) + S4.2 (PII) |
| M10 Verification | S1.4 |
| M11 Subagent Orchestration | S3.1 |
| M12 Init / Boot | S0.1 + S4.1 |
| Cross-cut R9 | S4.3 |
| Cross-cut Skill prefetch | S4.4 |

**Total**: 4 themed sessions + 1 smoke = **5 sessions / 18 sub-scenarios / 100% module coverage**.

---

## 6. Evidence Schema

### 6.1 Per-Scenario `evidence.json`

```jsonc
{
  "scenario_id": "S1.2",
  "module": "M6",
  "title": "Tool Calling Schema",
  "started_at": "2026-04-25T15:00:00Z",
  "duration_ms": 8421,
  "status": "pass",                  // pass | fail | skip | blocked
  "severity_on_fail": "critical",    // critical | high | medium | low
  "checks": [
    {
      "id": "schemars_validation",
      "kind": "trace_assertion",
      "query": "jq '.event==\"tool_call.validated\" and .valid==true' trace.jsonl | wc -l",
      "expected": ">= 1",
      "actual": "1",
      "passed": true
    },
    {
      "id": "file_written",
      "kind": "fs_assertion",
      "query": "test -f /tmp/aleph-test/fib.rs",
      "expected": "exit 0",
      "actual": "exit 0",
      "passed": true
    },
    {
      "id": "tool_not_found_structured_error",
      "kind": "response_assertion",
      "query": "grep '\"error_type\":\"tool_not_found\"' response_log.md",
      "expected": "1 hit, AgentLoop didn't crash",
      "actual": "1 hit, loop.end seen after error",
      "passed": true
    }
  ],
  "artifacts": ["trace.jsonl", "response_log.md", "session_events.csv"],
  "notes": ""
}
```

### 6.2 Six Check Kinds

| `kind` | Underlying tool | Use case |
|---|---|---|
| `trace_assertion` | `jq` over `trace.jsonl` | Harness internal events |
| `sql_assertion` | `sqlite3 state.db` | Persistent state changes |
| `log_grep` | `grep`/`rg` over server log | Free-text events |
| `fs_assertion` | `test`, `ls`, `find` | File system effects |
| `http_assertion` | `curl` against gateway REST | Live state queries |
| `response_assertion` | `grep` over recorded webchat response | Conversational behavior |

### 6.3 Validation Script Skeleton

```bash
#!/usr/bin/env bash
# scripts/validate-harness/S1.2-tool-calling-schema.sh
set -euo pipefail
source scripts/validate-harness/_lib.sh

SCN="S1.2"
MODULE="M6"
SEVERITY="critical"
EVIDENCE_DIR="${ALEPH_TEST_EVIDENCE_DIR:?}/$SCN"
mkdir -p "$EVIDENCE_DIR"

# Check 1
N=$(jq -r 'select(.event=="tool_call.validated" and .valid==true) | 1' \
        "$EVIDENCE_DIR/trace.jsonl" | wc -l | xargs)
check "schemars_validation" "trace_assertion" \
      ">= 1" "$N" "[[ $N -ge 1 ]]"

# Check 2
check "file_written" "fs_assertion" \
      "exit 0" "$(test -f /tmp/aleph-test/fib.rs && echo 0 || echo 1)" \
      "test -f /tmp/aleph-test/fib.rs"

# Check 3
HITS=$(grep -c '"error_type":"tool_not_found"' "$EVIDENCE_DIR/response_log.md" || true)
check "tool_not_found_structured" "response_assertion" \
      "1 hit + loop continued" "$HITS hit, loop.end present" \
      "[[ $HITS -ge 1 ]] && grep -q 'loop.end' $EVIDENCE_DIR/trace.jsonl"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

`_lib.sh` provides `check`, `fail`, `emit_evidence`. Exit codes:
- `0` — pass
- `1` — fail (any `check` failed)
- `2` — blocked (precondition not met, e.g., trace.jsonl missing)

### 6.4 Severity Tier Policy

| Sub-scenario | Severity on fail | On-fail behavior |
|---|---|---|
| S0.1, S0.2, S0.3 | **critical** | fail-fast (entire run halts) |
| S1.2 schema | **critical** | fail-fast |
| S1.3 sandbox | **critical** | fail-fast (security) |
| S3.3a transient retry | **critical** | fail-fast (misclassification = prod hang) |
| S3.3d unexpected supervisor | **critical** | fail-fast |
| S4.1 boot deep | **critical** | fail-fast |
| S4.2 PII | **critical** | fail-fast (security) |
| S1.4 stop hook | high | record, continue |
| S2.1 prompt assembly | high | record, continue |
| S2.2 memory | high | record, continue |
| S2.3 compaction | high | record, continue |
| S3.1 subagent fork | high | record, continue |
| S3.3b/c | high | record, continue |
| S4.3 R9 tool-as-config | high | record, continue |
| S2.4 JIT retrieval | medium | record |
| S3.2 replay e2e | medium | record (unit test covers core) |
| S4.4 skill prefetch | medium | record |

`run-all.sh` parses each script's `SEVERITY` and exit code, applies the tier policy.

### 6.5 Aggregated `REPORT.md`

```markdown
# Harness Production Validation Report

**Run**: 2026-04-25T15:00:00Z (UTC+8: 23:00)
**Build**: just build (release) — commit 039d11867
**Aleph version**: 2026.04.23
**Test HOME**: /tmp/aleph-validate-1745568000
**Real ~/.aleph/data**: unmodified (read-only copy)

## Verdict

| | Count | Status |
|---|---|---|
| 12-module coverage | 12/12 | ✅ |
| Sub-scenarios passed | 15/16 | 🟡 1 medium fail |
| Critical fails | 0 | ✅ |
| High fails | 0 | ✅ |
| Medium fails | 1 | 🟡 S2.4 JIT retrieval |
| Low fails | 0 | ✅ |

**Conclusion**: zero refactor regressions; 100% functional coverage; track S2.4 in a follow-up issue.

## 12-Module Matrix
| Module | Scenarios | Status | Key Evidence |
| ... | ... | ... | ... |

## Per-Scenario Detail
### S1.2 — Tool Calling Schema (M6, severity=critical)
**Status**: ✅ Pass · **Duration**: 8.4s · [trace.jsonl](S1.2/trace.jsonl)
- ✅ schemars_validation
- ✅ file_written
- ✅ tool_not_found_structured

### S2.4 — JIT Retrieval (M3+M4, severity=medium) ← FAILED
**Status**: ❌ Fail · **Duration**: 12.1s · [trace.jsonl](S2.4/trace.jsonl)
- ✅ memory.retrieve event present
- ❌ chunk_id contains "fib": expected ≥1 hit, got 0
- Investigation: check long-term memory writer in S2.2 — was author info persisted? Or is retrieve query embedding similarity below threshold?

...
```

---

## 7. Execution Flow

### 7.1 Pre-Flight (one-time, ~15 min)

1. `pkill -f aleph-server` + `sleep 2` + verify zero processes — **CLAUDE.md redline; vault is destroyed if multiple aleph-servers race for `.shared_token`**
2. `lsof -i:8080 -i:9090` — abort if either port is occupied
3. `just build` — release build with Swift bridge + WASM (~10 min)
4. `ALEPH_TEST_HOME=/tmp/aleph-validate-$(date +%s)` — fresh isolation root
5. `mkdir -p $ALEPH_TEST_HOME/.aleph/{data,logs}`
6. Copy real → test: `vault*`, `.shared_token`, `config.toml`, `providers/`, `skills/` (read-only `cp`)
7. `HOME=$ALEPH_TEST_HOME ALEPH_TRACE_FILE=$EVIDENCE_DIR/trace.jsonl ./target/release/aleph-server start &`
8. Poll `/health` up to 60s; abort on timeout
9. Extract boot init events into `$EVIDENCE_DIR/boot.log` (basis for S0.1 / S4.1)
10. Write `run-meta.json` (commit, version, timestamp, HOME, PID)

### 7.2 Conversation Loop ("Conductor Mode")

The user is the **webchat operator**. The assistant (Claude) is the **playbook conductor**. Per sub-scenario:

```
[Conductor] "S1.2 — paste this prompt in webchat: <text>"
   ↓
[Operator]  copy → webchat → wait for LLM response
   ↓
[Operator]  "done" (and paste key portions of response if needed)
   ↓
[Conductor] runs scripts/validate-harness/S1.2-*.sh
   ↓
[Conductor] reads evidence.json → reports ✅/❌ → next scenario or abort
```

**Why not full automation**: webchat is a browser UI. Driving it via Playwright is fragile and loses the "real conversation" flavor; bypassing it via direct gateway WS loses the "webchat" channel the user explicitly requested. Conductor mode preserves the real channel while keeping evidence collection automated.

**Special-case logistics**:
- **S2.3** — pre-prepared 10 long-form text blocks (~30k tokens total). Conductor presents them in a code block; operator pastes one at a time.
- **S3.2 replay** — fully side-channel via `curl`; operator does not interact.
- **S3.3a** — conductor `curl POST /providers/<id>/host` to break, then restore.
- **S3.3d** — conductor `kill -9 <subagent_pid>` to trigger supervisor restart; operator observes main session continues.
- **S4.4 slash menu** — operator types `/`, captures menu rendering (description suffices).

### 7.3 Failure Escalation

```
script exit 0 → ✅ → next scenario
script exit 1 → read SEVERITY:
  ├─ critical → ❌ STOP, write partial REPORT.md + investigation checklist → terminate
  ├─ high     → 🟡 record fail in REPORT.md, continue
  └─ medium/low → 🟡 record, continue
script exit 2 → ⏸️ blocked, mark and skip; final report lists blocked scenarios
```

Critical-failure partial REPORT.md template:

```markdown
# ⛔ Test Aborted (Critical Failure)

**Failed scenario**: S1.3 — Sandbox Guardrail
**Failed at**: 2026-04-25T15:23
**Passed before**: S0.1, S0.2, S0.3, S1.1, S1.2 (5/16)
**Not run**: S1.4, S2.*, S3.*, S4.* (11/16, marked blocked)

## Critical Evidence
[failed-check details]

## Investigation Path
1. ...
2. ...

## Restart Options
- A) Fix the bug, re-run S1.3 only (other passed scenarios remain valid)
- B) Full re-run (re-do pre-flight)
```

### 7.4 Post-Flight (~5 min)

1. Graceful shutdown: `kill -TERM $ALEPH_PID; sleep 3; kill -KILL $ALEPH_PID 2>/dev/null`
2. Extract notable events: `grep -E "ERROR|WARN|compaction|subagent|stop_hook|pii" server.log > notable-events.log`
3. Dump key SQLite tables to CSV: `session_events`, `memory_events`, `agent_events`, `traces`
4. Render `REPORT.md` via `scripts/validate-harness/render-report.py` (consumes all `evidence.json` files)
5. Confirm-prompt `rm -rf $ALEPH_TEST_HOME`
6. Audit real `~/.aleph/data` for mtime changes vs. `run-meta.json.started_at` — must be empty

---

## 8. Verification-Required (TBD list resolved during plan phase)

| Item | How plan-phase verifies | Fallback |
|---|---|---|
| **`ALEPH_TRACE_FILE` env var natively supported** | `grep -rn "ALEPH_TRACE\|TRACE_FILE\|trace_sink" src/harness/ src/observability/` | If not: plan adds a small patch making `trace_sink` read the env. Patch is reverted post-validation. |
| **Vault is file-based (not Keychain)** | `grep -rn "Security.framework\|Keychain\|SecItem" src/vault/ src/secrets/` | If Keychain-based: HOME redirect won't isolate vault; revert to data isolation strategy ii (backup + run on real `~/.aleph/data` + restore). |
| **Gateway `POST /sessions/{id}/replay` exposed** | `grep -rn "/replay\|sessions/.*replay" src/gateway/routes/ src/bin/` | If not exposed: S3.2 falls back to a small ad-hoc binary that loads `SessionEventStore` directly; runs side-channel against `state.db`. |
| **Stop-hook registration API surface** | `grep -rn "stop_hook\|StopHook" src/gateway/routes/ src/verification/` to find the registration entry-point used in S1.4 | If no public API: register the hook by writing directly to `~/.aleph/data/config.toml` `[verification.stop_hooks]` before pre-flight starts the server, so the hook is loaded at boot. |
| **`trace_sink` supports concurrent JSONL writes** | Read `src/harness/trace_sink.rs` impl | If not: shard by session_id; `evidence.json.artifacts` paths become `trace-<sid>.jsonl`. |
| **macOS TCC permissions are binary-path bound** | Run `screen.capture` tool from test-HOME aleph; observe `PermissionDenied` | If yes: skip desktop-capability tools in S1/S4; mark them as "skip-tcc" in evidence; this does not block 12-module coverage (none of M1–M12 require desktop capability). |

These items do not block design approval. They are concrete probes the writing-plans skill must complete before implementation.

---

## 9. File Layout

```
target/test-evidence/                        # gitignored
├── REPORT.md                                # aggregated final report
├── run-meta.json                            # commit, version, timestamp, HOME, PID
├── boot.log                                 # boot-phase events (basis for S0.1, S4.1)
├── notable-events.log                       # post-flight grep
├── db-session_events.csv                    # post-flight SQLite dumps
├── db-memory_events.csv
├── db-agent_events.csv
├── db-traces.csv
├── S0.1/
│   ├── evidence.json
│   └── boot-init-events.log
├── S0.2/...
├── S1.1/
│   ├── evidence.json
│   ├── trace.jsonl
│   └── response_log.md                      # webchat conversation transcript
├── ...
└── S4.4/...

scripts/validate-harness/
├── _lib.sh                                  # check / fail / emit_evidence
├── preflight.sh                             # §7.1 steps 1–10
├── postflight.sh                            # §7.4 steps 1–6
├── run-all.sh                               # tier-aware orchestrator
├── render-report.py                         # evidence.json → REPORT.md
├── S0.1-boot-health.sh
├── S0.2-ws-auth.sh
├── S0.3-think-act-loop.sh
├── S1.1-tool-discovery.sh
├── S1.2-tool-calling-schema.sh
├── S1.3-sandbox-guardrail.sh
├── S1.4-stop-hook.sh
├── S2.1-prompt-assembly-layers.sh
├── S2.2-memory-write-recall.sh
├── S2.3-context-compaction.sh
├── S2.4-jit-retrieval.sh
├── S3.1-subagent-fork.sh
├── S3.2-checkpoint-replay.sh
├── S3.3-error-handling-4class.sh
├── S4.1-boot-deep.sh
├── S4.2-pii-guardrail.sh
├── S4.3-tool-as-config.sh
└── S4.4-skill-prefetch.sh

docs/superpowers/specs/
└── 2026-04-25-harness-production-validation-design.md   # this document
```

`target/test-evidence/` is gitignored (already covered by `target/`). `scripts/validate-harness/` is checked in — the playbook is reusable on any future commit.

---

## 10. Out of Scope (YAGNI)

- ❌ HTML dashboard for the report — Markdown is sufficient and diff-friendly.
- ❌ Cross-run regression diffing — future work; the schema is forward-compatible.
- ❌ Webchat browser automation — preserves the real-conversation requirement.
- ❌ Replacement of unit tests — those still run as baseline.
- ❌ Continuous re-runs / scheduled CI — manual run only this round.
- ❌ Multi-platform (Windows/Linux) validation — macOS-only this round; the test rig design is portable but not validated elsewhere.

---

## 11. References

- **Roadmap**: [2026-04-24-harness-dissolution-roadmap.md](./2026-04-24-harness-dissolution-roadmap.md) (P0–P7 complete)
- **Architectural redlines**: `CLAUDE.md` R1–R10 (especially R3 Core Minimalism, R8 LLM Sovereignty, R10 Intelligence in Prompt)
- **Process Management redline**: `CLAUDE.md` "进程管理" (vault destroyed by multi-instance race)
- **Boot assembly reference**: `docs/reference/BOOT_ASSEMBLY.md` (P6 deliverable)
- **State layer reference**: `docs/reference/STATE_LAYER.md` (P7 deliverable)
- **Gateway reference**: `docs/reference/GATEWAY.md`
- **Context budget reference**: `docs/reference/AGENT_LOOP_CONTEXT_BUDGET.md`
- **Source article**: `/Volumes/TBU4/Agent-Harness.md` (12-module ontology)
