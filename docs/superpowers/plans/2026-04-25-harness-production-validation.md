# Harness Production Validation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a re-runnable end-to-end test rig that exercises all 12 modules of the Agent Harness ontology through real webchat conversation + side-channel evidence collection, then execute it against the post-dissolution Aleph (commit 039d11867+) to produce a `target/test-evidence/REPORT.md` proving zero regressions.

**Architecture:** Dual-channel design — `HOME=/tmp/aleph-validate-...` redirected aleph-server runs the conversation through browser webchat (channel A); bash + jq + sqlite3 + curl validation scripts collect objective evidence (channel B). Both authenticated by gateway token `aleph-9976129a-407d-4893-a96c-6467b24bedac`. Tier-aware failure policy: critical → fail-fast, high/medium/low → record-and-continue.

**Tech Stack:** bash 5.x, jq, sqlite3, curl, python3 (report renderer only), Aleph release binary (just build), browser webchat (Trunk WASM).

**Spec:** [docs/superpowers/specs/2026-04-25-harness-production-validation-design.md](../specs/2026-04-25-harness-production-validation-design.md)

---

## Phase A — Verification-Required Resolution (6 tasks)

These resolve the §8 TBD items in the spec. Each is a small probe; if the assumed fact is wrong, fall back to the documented alternative. Run before any rig code is written — the answers shape `preflight.sh` and the scenario scripts.

### Task 1: Verify `ALEPH_TRACE_FILE` env var support

**Files:**
- Read-only audit: `src/harness/trace_sink.rs`, `src/harness/trace.rs`, `src/observability/`
- Modify (only if missing): `src/harness/trace_sink.rs` (add env-var read)

- [ ] **Step 1: Audit current trace_sink for env-var support**

```bash
grep -rn "ALEPH_TRACE\|TRACE_FILE\|trace_file\|trace_path" src/harness/ src/observability/ 2>/dev/null
ls src/harness/trace_sink.rs src/harness/trace.rs 2>/dev/null
```

Expected: locate the trait `TraceSink` and any concrete `FileTraceSink` / `JsonlTraceSink` impl.

- [ ] **Step 2: Read the trace_sink.rs implementation**

Read `src/harness/trace_sink.rs` end-to-end. Look for:
- Does it have a constructor that takes a `PathBuf`?
- Is it wired in boot (search for `TraceSink::new` / `.with_trace_sink(`)?
- Where does the path come from today (config-only or env-aware)?

- [ ] **Step 3: Decision branch**

If env-var already supported: record finding in `Phase-A-findings.md` (next step), no code change.

If not supported: write a minimal patch — `std::env::var("ALEPH_TRACE_FILE").ok()` as override at the construction site. Patch must be ≤ 10 lines.

- [ ] **Step 4: Write Phase A findings file**

Create `docs/superpowers/plans/Phase-A-findings.md`:

```markdown
# Phase A — Verification-Required Findings

## TBD#1: ALEPH_TRACE_FILE env var
- Pre-existing: yes/no
- Patch required: yes (≤ 10 lines in `src/harness/trace_sink.rs:NN`) / no
- Patch reverted post-validation: yes / N/A
```

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/plans/Phase-A-findings.md src/harness/trace_sink.rs
git commit -m "validate: TBD#1 trace env-var support"
```

---

### Task 2: Verify vault is file-based (not Keychain)

- [ ] **Step 1: Grep for Keychain usage**

```bash
grep -rn "Security.framework\|Keychain\|SecItem\|kSecClass\|secret-service" src/vault/ src/secrets/ src/cred*/ 2>/dev/null
```

Expected: zero hits (file-based).

- [ ] **Step 2: Read vault initialization**

```bash
grep -rn "vault\.\(open\|new\|load\)\|Vault::\(new\|open\|load\)" src/vault/ src/bin/aleph-server/ 2>/dev/null | head -10
```

Confirm the vault opens a file relative to `dirs::home_dir()` (validates HOME redirect works).

- [ ] **Step 3: Append to Phase-A-findings.md**

```markdown
## TBD#2: vault storage
- Storage type: file-based / keychain
- HOME redirect viable: yes / no
- Fallback needed: no / data-isolation strategy ii (backup + restore)
```

- [ ] **Step 4: Commit**

```bash
git add docs/superpowers/plans/Phase-A-findings.md
git commit -m "validate: TBD#2 vault storage type"
```

---

### Task 3: Verify gateway `POST /sessions/{id}/replay` exposure

- [ ] **Step 1: Grep gateway routes**

```bash
grep -rn "/replay\|sessions/.*replay\|replay_handler\|/sessions/:id" src/gateway/routes/ src/gateway/handlers/ src/bin/aleph-server/ 2>/dev/null | head -20
```

- [ ] **Step 2: Inspect SessionActor::replay public surface**

```bash
grep -rn "pub.*fn replay\|pub.*async fn replay" src/session/actor.rs src/session/store.rs 2>/dev/null
```

- [ ] **Step 3: Decision**

If REST endpoint exposed: S3.2 uses curl. Document URL + payload schema in findings.

If not exposed: write a minimal `src/bin/aleph-replay-probe.rs` that takes `--session-id --from --to` args, opens the same SQLite DB, calls `SessionEventStore::load` + `SessionActor::replay`, prints resulting `head_seq` + state hash. Used by S3.2 instead of curl.

- [ ] **Step 4: Append to findings + commit**

```markdown
## TBD#3: gateway replay endpoint
- Exposed: yes (POST /api/sessions/<id>/replay) / no
- S3.2 implementation: curl / aleph-replay-probe binary
```

```bash
git add docs/superpowers/plans/Phase-A-findings.md src/bin/aleph-replay-probe.rs
git commit -m "validate: TBD#3 replay endpoint"
```

---

### Task 4: Verify stop-hook registration surface

- [ ] **Step 1: Grep stop-hook public registration**

```bash
grep -rn "stop_hook\|StopHook\|register_stop" src/gateway/routes/ src/verification/ src/config/schema.rs 2>/dev/null | head -20
```

- [ ] **Step 2: Inspect config-loaded hooks**

Look for `[verification.stop_hooks]` or similar in `src/config/schema.rs` and example configs in `docs/`. Confirm whether `~/.aleph/data/config.toml` `[verification]` section accepts hooks at boot.

- [ ] **Step 3: Decision**

If runtime registration API exists: S1.4 uses it (capture endpoint + payload).

If only config-time loading: S1.4 writes the hook into `$ALEPH_TEST_HOME/.aleph/data/config.toml` during pre-flight, before aleph starts. Document the TOML schema.

- [ ] **Step 4: Append findings + commit**

```markdown
## TBD#4: stop-hook registration
- Runtime API: yes / no
- S1.4 strategy: POST /api/.../hooks  /  config.toml [verification.stop_hooks]
- Hook spec: { "id": "...", "match_pattern": "TODO:", "action": "stop" }
```

```bash
git add docs/superpowers/plans/Phase-A-findings.md
git commit -m "validate: TBD#4 stop-hook registration"
```

---

### Task 5: Verify trace_sink concurrent-write safety

- [ ] **Step 1: Read trace_sink impl for locking**

```bash
grep -n "Mutex\|RwLock\|tokio::sync\|file.lock\|append" src/harness/trace_sink.rs 2>/dev/null
```

- [ ] **Step 2: Decision**

If single-file with mutex: OK; one `trace.jsonl` for whole run.

If race-prone or per-session sharded already: evidence schema uses `trace-<session_id>.jsonl`; the scenario scripts grep against the right shard.

- [ ] **Step 3: Append findings + commit**

```markdown
## TBD#5: trace concurrent writes
- Concurrency model: mutex-guarded / sharded per session / unsafe (race)
- Evidence file layout: single trace.jsonl / trace-<sid>.jsonl
- Each scenario script reads from: <path>
```

```bash
git add docs/superpowers/plans/Phase-A-findings.md
git commit -m "validate: TBD#5 trace concurrency"
```

---

### Task 6: Verify macOS TCC binary-path binding for desktop tools

- [ ] **Step 1: Identify desktop-capability tools in registry**

```bash
grep -rn "screen.capture\|audio.record\|hotkey\|accessibility" src/builtin_tools/ src/desktop/ 2>/dev/null | head -10
```

- [ ] **Step 2: Read TCC handling in bridge**

```bash
grep -rn "TCC\|kAuthorizationRule\|requestAccess" desktop/macos/bridge/Sources/ 2>/dev/null | head -10
```

- [ ] **Step 3: Decision**

TCC is bound to the binary path **and** code signature. Running `target/release/aleph-server` with redirected `$HOME` does **not** change the binary path. So existing TCC grants apply — desktop tools should function. But the test rig **does not exercise** desktop tools in S0–S4 by design (none of M1–M12 require desktop capability).

If audit reveals a scenario accidentally hits a desktop tool: mark it `skip-tcc` in evidence; not a test failure.

- [ ] **Step 4: Append findings + commit**

```markdown
## TBD#6: macOS TCC binding
- Binding: binary path + signature (HOME redirect transparent)
- Test scenarios touching desktop tools: none by design
- skip-tcc fallback: documented for future
```

```bash
git add docs/superpowers/plans/Phase-A-findings.md
git commit -m "validate: TBD#6 macOS TCC"
```

---

## Phase B — Shared Infrastructure (5 tasks)

### Task 7: Build `_lib.sh` (the heart of the rig)

**Files:**
- Create: `scripts/validate-harness/_lib.sh`
- Test: `scripts/validate-harness/test-lib.sh`

- [ ] **Step 1: Write the test harness**

Create `scripts/validate-harness/test-lib.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"

# Test 1: check passes when condition true
ALEPH_TEST_EVIDENCE_DIR=/tmp/lib-test-$$
mkdir -p "$ALEPH_TEST_EVIDENCE_DIR/TST"
check "always_true" "fs_assertion" "exit 0" "exit 0" "true"
[[ "${LAST_CHECK_RESULT:-}" == "pass" ]] || { echo "FAIL: check should pass"; exit 1; }

# Test 2: check records failure but doesn't exit
check "always_false" "fs_assertion" "exit 0" "exit 1" "false"
[[ "${LAST_CHECK_RESULT:-}" == "fail" ]] || { echo "FAIL: check should record fail"; exit 1; }

# Test 3: emit_evidence writes valid JSON
emit_evidence "TST" "M99" "low" "TST"
jq empty "$ALEPH_TEST_EVIDENCE_DIR/TST/evidence.json" || { echo "FAIL: invalid JSON"; exit 1; }

# Test 4: status reflects all-pass / any-fail
STATUS=$(jq -r .status "$ALEPH_TEST_EVIDENCE_DIR/TST/evidence.json")
[[ "$STATUS" == "fail" ]] || { echo "FAIL: status should be fail (1 check failed)"; exit 1; }

rm -rf "$ALEPH_TEST_EVIDENCE_DIR"
echo "_lib.sh tests: PASS"
```

- [ ] **Step 2: Run the test, expect failure (lib not yet written)**

```bash
bash scripts/validate-harness/test-lib.sh
```

Expected: error sourcing `_lib.sh` (file not found).

- [ ] **Step 3: Implement `_lib.sh`**

Create `scripts/validate-harness/_lib.sh`:

```bash
#!/usr/bin/env bash
# Shared helpers for harness validation scripts.
# Source from each S<N>.<M>-<slug>.sh.

# Globals populated per-scenario:
#   ALEPH_TEST_EVIDENCE_DIR  — root evidence dir (must be set by caller)
#   _CHECKS_JSON             — jq-array of check objects (built up by check())
#   LAST_CHECK_RESULT        — "pass" | "fail" (for in-script branching/tests)
_CHECKS_JSON='[]'

# check <id> <kind> <expected> <actual> <test_expr>
#   <test_expr> is a bash expression evaluated; success = pass.
check() {
  local id="$1" kind="$2" expected="$3" actual="$4" test_expr="$5"
  local passed
  if eval "$test_expr"; then passed=true; LAST_CHECK_RESULT=pass
  else                       passed=false; LAST_CHECK_RESULT=fail
  fi
  _CHECKS_JSON=$(jq --arg id "$id" --arg kind "$kind" \
                    --arg exp "$expected" --arg act "$actual" \
                    --argjson p "$passed" \
                    '. + [{id:$id,kind:$kind,expected:$exp,actual:$act,passed:$p}]' \
                    <<<"$_CHECKS_JSON")
}

# fail <reason>  — emit a synthetic failed check + exit 1
fail() {
  check "_fatal" "log_grep" "no fatal" "$1" "false"
  emit_evidence "${SCN:-???}" "${MODULE:-???}" "${SEVERITY:-critical}" "${SCN:-???}"
  exit 1
}

# emit_evidence <scn_id> <module> <severity> <subdir>
emit_evidence() {
  local scn="$1" module="$2" severity="$3" subdir="${4:-$1}"
  local out_dir="$ALEPH_TEST_EVIDENCE_DIR/$subdir"
  mkdir -p "$out_dir"
  local started_at="${SCN_STARTED_AT:-$(date -u +%FT%TZ)}"
  local now
  now=$(date -u +%FT%TZ)
  local fails
  fails=$(jq '[.[] | select(.passed==false)] | length' <<<"$_CHECKS_JSON")
  local status
  if [[ "$fails" -eq 0 ]]; then status=pass; else status=fail; fi

  jq -n --arg id "$scn" --arg mod "$module" --arg sev "$severity" \
        --arg started "$started_at" --arg ended "$now" \
        --arg status "$status" \
        --argjson checks "$_CHECKS_JSON" \
        '{
          scenario_id: $id,
          module: $mod,
          started_at: $started,
          ended_at: $ended,
          status: $status,
          severity_on_fail: $sev,
          checks: $checks
        }' > "$out_dir/evidence.json"

  if [[ "$status" == "pass" ]]; then exit 0; else exit 1; fi
}
```

- [ ] **Step 4: Run the test — expect pass**

```bash
chmod +x scripts/validate-harness/{_lib,test-lib}.sh
bash scripts/validate-harness/test-lib.sh
```

Expected: `_lib.sh tests: PASS`

- [ ] **Step 5: Commit**

```bash
git add scripts/validate-harness/_lib.sh scripts/validate-harness/test-lib.sh
git commit -m "validate: shared bash helpers (_lib.sh) with self-tests"
```

---

### Task 8: Build `preflight.sh`

**Files:**
- Create: `scripts/validate-harness/preflight.sh`

- [ ] **Step 1: Write preflight.sh**

```bash
#!/usr/bin/env bash
# Pre-flight: kill aleph, build, set up isolated HOME, start aleph, wait health.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# 1. Kill aleph (CLAUDE.md redline)
pkill -f "target/release/aleph-server" 2>/dev/null || true
pkill -f "target/debug/aleph-server"   2>/dev/null || true
sleep 2
if ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail >/dev/null; then
  echo "FATAL: aleph processes still alive; refuse to continue" >&2
  ps aux | grep "[a]leph-server" | grep -v zsh
  exit 1
fi

# 2. Port collision check
for port in 8080 9090; do
  if lsof -i:$port >/dev/null 2>&1; then
    echo "FATAL: port $port is in use" >&2
    lsof -i:$port
    exit 1
  fi
done

# 3. Release build (~10 min)
echo "==> just build"
just build

# 4. Test HOME
export ALEPH_TEST_HOME="/tmp/aleph-validate-$(date +%s)"
mkdir -p "$ALEPH_TEST_HOME/.aleph/data" "$ALEPH_TEST_HOME/.aleph/logs"
export ALEPH_TEST_EVIDENCE_DIR="$REPO_ROOT/target/test-evidence"
mkdir -p "$ALEPH_TEST_EVIDENCE_DIR"

# 5. Copy real → test (read-only on source)
SRC="$HOME/.aleph/data"
DST="$ALEPH_TEST_HOME/.aleph/data"
for item in vault vault.bin .shared_token config.toml providers skills agents; do
  if [[ -e "$SRC/$item" ]]; then cp -R "$SRC/$item" "$DST/"; fi
done
echo "==> copied $(ls "$DST" | wc -l) items into test HOME"

# 6. Start aleph with HOME redirect + trace env
LOG="$ALEPH_TEST_HOME/.aleph/logs/aleph-server.log"
HOME="$ALEPH_TEST_HOME" \
  ALEPH_TRACE_FILE="$ALEPH_TEST_EVIDENCE_DIR/trace.jsonl" \
  ./target/release/aleph-server start \
  > "$LOG" 2>&1 &
ALEPH_PID=$!
echo "$ALEPH_PID" > "$ALEPH_TEST_EVIDENCE_DIR/.aleph.pid"
echo "==> aleph started (pid=$ALEPH_PID)"

# 7. Wait for /health
echo "==> waiting for /health (60s timeout)"
for i in $(seq 1 60); do
  if curl -sf http://127.0.0.1:9090/health >/dev/null 2>&1; then
    echo "==> health OK after ${i}s"
    break
  fi
  sleep 1
  if [[ "$i" -eq 60 ]]; then
    echo "FATAL: /health timeout" >&2
    tail -50 "$LOG" >&2
    kill -KILL "$ALEPH_PID" 2>/dev/null || true
    exit 1
  fi
done

# 8. Extract boot log for S0.1 / S4.1
grep -E "boot\.module\.init|boot\.complete|boot\.phase" "$LOG" \
  > "$ALEPH_TEST_EVIDENCE_DIR/boot.log" || true

# 9. Write run-meta
cat > "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json" <<EOF
{
  "started_at": "$(date -u +%FT%TZ)",
  "build_commit": "$(git rev-parse HEAD)",
  "aleph_version": "$(cat VERSION)",
  "test_home": "$ALEPH_TEST_HOME",
  "evidence_dir": "$ALEPH_TEST_EVIDENCE_DIR",
  "gateway_token_prefix": "aleph-9976",
  "aleph_pid": $ALEPH_PID
}
EOF

# 10. Print operator instructions
cat <<EOF

==========================================================
PRE-FLIGHT COMPLETE.
Test HOME : $ALEPH_TEST_HOME
Evidence  : $ALEPH_TEST_EVIDENCE_DIR
PID       : $ALEPH_PID
Log       : $LOG
Health    : http://127.0.0.1:9090/health (200)

Now open browser:
  http://127.0.0.1:8080/?token=aleph-9976129a-407d-4893-a96c-6467b24bedac

Then run scenarios in order:
  bash scripts/validate-harness/run-all.sh
==========================================================
EOF
```

- [ ] **Step 2: Make executable + sanity-check**

```bash
chmod +x scripts/validate-harness/preflight.sh
bash -n scripts/validate-harness/preflight.sh   # syntax check only
```

Expected: no output (parse OK).

- [ ] **Step 3: Commit**

```bash
git add scripts/validate-harness/preflight.sh
git commit -m "validate: preflight orchestrator"
```

---

### Task 9: Build `postflight.sh`

**Files:**
- Create: `scripts/validate-harness/postflight.sh`

- [ ] **Step 1: Write postflight.sh**

```bash
#!/usr/bin/env bash
# Post-flight: stop aleph, dump artifacts, render report, optional cleanup.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
EVIDENCE_DIR="${ALEPH_TEST_EVIDENCE_DIR:-$REPO_ROOT/target/test-evidence}"

# Resolve HOME from run-meta
TEST_HOME=$(jq -r .test_home "$EVIDENCE_DIR/run-meta.json")
PID=$(jq -r .aleph_pid "$EVIDENCE_DIR/run-meta.json")
LOG="$TEST_HOME/.aleph/logs/aleph-server.log"

# 1. Graceful shutdown
echo "==> stopping aleph (pid=$PID)"
kill -TERM "$PID" 2>/dev/null || true
sleep 3
kill -KILL "$PID" 2>/dev/null || true

# 2. Notable events extract
grep -E "ERROR|WARN|compaction|subagent|stop_hook|pii|sandbox\.denied" "$LOG" \
  > "$EVIDENCE_DIR/notable-events.log" || true

# 3. SQLite dumps
DB="$TEST_HOME/.aleph/data/state.db"
if [[ -f "$DB" ]]; then
  for table in session_events memory_events agent_events traces; do
    sqlite3 "$DB" -header -csv "SELECT * FROM $table" \
      > "$EVIDENCE_DIR/db-$table.csv" 2>/dev/null || true
  done
fi

# 4. Render REPORT.md
python3 "$SCRIPT_DIR/render-report.py" \
  --evidence-dir "$EVIDENCE_DIR" \
  --output "$EVIDENCE_DIR/REPORT.md"
echo "==> REPORT.md written"

# 5. Pollution audit on real HOME
START_AT=$(jq -r .started_at "$EVIDENCE_DIR/run-meta.json")
START_EPOCH=$(date -j -f "%Y-%m-%dT%H:%M:%SZ" "$START_AT" +%s 2>/dev/null || \
              date -d "$START_AT" +%s)
NEW=$(find "$HOME/.aleph/data" -newer "$EVIDENCE_DIR/run-meta.json" 2>/dev/null | head -10)
if [[ -z "$NEW" ]]; then
  echo "==> ✅ real ~/.aleph/data unmodified"
else
  echo "==> ⚠️  real ~/.aleph/data has changes since test start:"
  echo "$NEW"
fi

# 6. Cleanup (interactive)
echo
read -p "Delete test HOME $TEST_HOME ? (y/N) " yn
[[ "$yn" == "y" || "$yn" == "Y" ]] && rm -rf "$TEST_HOME" && echo "==> deleted"

echo
echo "==> Post-flight complete. See $EVIDENCE_DIR/REPORT.md"
```

- [ ] **Step 2: Sanity-check + commit**

```bash
chmod +x scripts/validate-harness/postflight.sh
bash -n scripts/validate-harness/postflight.sh
git add scripts/validate-harness/postflight.sh
git commit -m "validate: postflight (stop, dump, render, audit)"
```

---

### Task 10: Build `run-all.sh` (tier-aware orchestrator)

**Files:**
- Create: `scripts/validate-harness/run-all.sh`

- [ ] **Step 1: Write run-all.sh**

```bash
#!/usr/bin/env bash
# Run scenarios in order. Tier policy:
#   exit 0           → ✅, next
#   exit 1 critical  → ❌ STOP, partial REPORT
#   exit 1 high/med  → 🟡 record, continue
#   exit 2           → ⏸️  blocked, skip
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EVIDENCE_DIR="${ALEPH_TEST_EVIDENCE_DIR:?must be set}"

SCENARIOS=(
  "S0.1-boot-health"
  "S0.2-ws-auth"
  "S0.3-think-act-loop"
  "S1.1-tool-discovery"
  "S1.2-tool-calling-schema"
  "S1.3-sandbox-guardrail"
  "S1.4-stop-hook"
  "S2.1-prompt-assembly-layers"
  "S2.2-memory-write-recall"
  "S2.3-context-compaction"
  "S2.4-jit-retrieval"
  "S3.1-subagent-fork"
  "S3.2-checkpoint-replay"
  "S3.3-error-handling-4class"
  "S4.1-boot-deep"
  "S4.2-pii-guardrail"
  "S4.3-tool-as-config"
  "S4.4-skill-prefetch"
)

PASSED=() FAILED_CRIT=() FAILED_OTHER=() BLOCKED=()

for scn in "${SCENARIOS[@]}"; do
  echo "──────── $scn ────────"
  read -p "Conductor: when sub-scenario conversation is complete in webchat, press [Enter] to validate (or 'skip', 'abort'): " action
  case "$action" in
    skip)  BLOCKED+=("$scn"); continue ;;
    abort) echo "==> aborted by operator"; break ;;
  esac

  bash "$SCRIPT_DIR/$scn.sh"
  rc=$?
  case $rc in
    0) PASSED+=("$scn") ;;
    1)
      sev=$(jq -r .severity_on_fail "$EVIDENCE_DIR/${scn%%-*}/evidence.json" 2>/dev/null || echo unknown)
      if [[ "$sev" == "critical" ]]; then
        FAILED_CRIT+=("$scn")
        echo "❌ CRITICAL fail in $scn — STOPPING"
        break
      else
        FAILED_OTHER+=("$scn ($sev)")
        echo "🟡 $sev fail in $scn — continuing"
      fi
      ;;
    2) BLOCKED+=("$scn") ;;
    *) echo "⚠️  unexpected rc=$rc"; FAILED_OTHER+=("$scn (rc=$rc)") ;;
  esac
done

echo
echo "==== SUMMARY ===="
echo "  passed       : ${#PASSED[@]}"
echo "  failed crit  : ${#FAILED_CRIT[@]}  ${FAILED_CRIT[*]}"
echo "  failed other : ${#FAILED_OTHER[@]}  ${FAILED_OTHER[*]}"
echo "  blocked      : ${#BLOCKED[@]}  ${BLOCKED[*]}"
echo
echo "Run scripts/validate-harness/postflight.sh to render REPORT.md"
```

- [ ] **Step 2: Sanity-check + commit**

```bash
chmod +x scripts/validate-harness/run-all.sh
bash -n scripts/validate-harness/run-all.sh
git add scripts/validate-harness/run-all.sh
git commit -m "validate: tier-aware run-all orchestrator"
```

---

### Task 11: Build `render-report.py`

**Files:**
- Create: `scripts/validate-harness/render-report.py`

- [ ] **Step 1: Write the renderer**

```python
#!/usr/bin/env python3
"""Aggregate evidence.json files into REPORT.md."""
import argparse
import json
import sys
from pathlib import Path

MODULE_LABELS = {
    "M1": "Orchestration Loop", "M2": "Tools", "M3": "Memory",
    "M4": "Context Management", "M5": "Prompt Assembly",
    "M6": "Tool Calling Schema", "M7": "State / Checkpoint",
    "M8": "Error Handling", "M9": "Guardrails",
    "M10": "Verification", "M11": "Subagent Orchestration",
    "M12": "Init / Boot",
}

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--evidence-dir", required=True, type=Path)
    ap.add_argument("--output", required=True, type=Path)
    args = ap.parse_args()

    meta = json.loads((args.evidence_dir / "run-meta.json").read_text())
    scenarios = []
    for ej in sorted(args.evidence_dir.glob("*/evidence.json")):
        scenarios.append(json.loads(ej.read_text()))

    by_status = {"pass": [], "fail": [], "skip": [], "blocked": []}
    by_severity_fail = {"critical": 0, "high": 0, "medium": 0, "low": 0}
    for s in scenarios:
        by_status[s["status"]].append(s)
        if s["status"] == "fail":
            by_severity_fail[s["severity_on_fail"]] += 1

    lines = []
    lines.append(f"# Harness Production Validation Report\n")
    lines.append(f"**Run**: {meta['started_at']}")
    lines.append(f"**Build**: just build (release) — commit `{meta['build_commit']}`")
    lines.append(f"**Aleph version**: {meta['aleph_version']}")
    lines.append(f"**Test HOME**: `{meta['test_home']}`\n")
    lines.append("## Verdict\n")
    lines.append(f"- Sub-scenarios: passed {len(by_status['pass'])} / {len(scenarios)}")
    lines.append(f"- Critical fails: {by_severity_fail['critical']}")
    lines.append(f"- High fails: {by_severity_fail['high']}")
    lines.append(f"- Medium fails: {by_severity_fail['medium']}")
    lines.append(f"- Low fails: {by_severity_fail['low']}\n")

    # Module matrix
    by_module = {}
    for s in scenarios:
        by_module.setdefault(s["module"], []).append(s)
    lines.append("## 12-Module Matrix\n")
    lines.append("| Module | Scenarios | Status |")
    lines.append("|---|---|---|")
    for mid in sorted(by_module.keys()):
        scns = by_module[mid]
        statuses = {s["status"] for s in scns}
        emoji = "✅" if statuses == {"pass"} else "❌" if "fail" in statuses else "⏸️"
        ids = ", ".join(s["scenario_id"] for s in scns)
        label = MODULE_LABELS.get(mid, mid)
        lines.append(f"| {mid} {label} | {ids} | {emoji} |")
    lines.append("")

    # Per-scenario detail
    lines.append("## Per-Scenario Detail\n")
    for s in scenarios:
        ic = "✅" if s["status"] == "pass" else "❌" if s["status"] == "fail" else "⏸️"
        lines.append(f"### {s['scenario_id']} — {s['module']} (severity={s['severity_on_fail']})  {ic}")
        for c in s["checks"]:
            mark = "✅" if c["passed"] else "❌"
            lines.append(f"- {mark} **{c['id']}** ({c['kind']}): expected `{c['expected']}`, got `{c['actual']}`")
        lines.append("")

    args.output.write_text("\n".join(lines))
    return 0

if __name__ == "__main__":
    sys.exit(main())
```

- [ ] **Step 2: Smoke-test with empty evidence dir**

```bash
mkdir -p /tmp/render-smoke
echo '{"started_at":"now","build_commit":"abc","aleph_version":"x","test_home":"/tmp"}' \
  > /tmp/render-smoke/run-meta.json
python3 scripts/validate-harness/render-report.py \
  --evidence-dir /tmp/render-smoke --output /tmp/render-smoke/REPORT.md
cat /tmp/render-smoke/REPORT.md | head -10
```

Expected: produces a minimal REPORT with 0 scenarios.

- [ ] **Step 3: Commit**

```bash
git add scripts/validate-harness/render-report.py
git commit -m "validate: REPORT.md renderer"
```

---

## Phase C — Scenario Scripts (5 tasks, one per session)

Each task builds the scripts for one session. Pattern is identical: source `_lib.sh`, set `SCN/MODULE/SEVERITY`, call `check` N times against the evidence files (trace.jsonl, server.log, state.db, response_log.md), then `emit_evidence`. The conductor's job (during the live run) is to capture `response_log.md` per scenario by pasting the webchat transcript.

### Task 12: S0 — Smoke Prelude (3 scripts)

**Files:**
- Create: `scripts/validate-harness/S0.1-boot-health.sh`
- Create: `scripts/validate-harness/S0.2-ws-auth.sh`
- Create: `scripts/validate-harness/S0.3-think-act-loop.sh`

- [ ] **Step 1: Write S0.1 — boot health**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S0.1"; MODULE="M12"; SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

# 1. /health returns 200
HC=$(curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:9090/health)
check "health_200" "http_assertion" "200" "$HC" "[[ $HC == 200 ]]"

# 2. boot.log has ≥ 12 init events
BOOT_LOG="$ALEPH_TEST_EVIDENCE_DIR/boot.log"
N=$(grep -c "boot\.module\.init" "$BOOT_LOG" 2>/dev/null || echo 0)
check "boot_init_count" "log_grep" ">= 12" "$N" "[[ $N -ge 12 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 2: Write S0.2 — ws-auth**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S0.2"; MODULE="gateway"; SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

LOG=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/logs/aleph-server.log"
TOK_TAIL="bedac"
N=$(grep -c "gateway\.ws\.authenticated.*$TOK_TAIL" "$LOG" 2>/dev/null || echo 0)
check "ws_authenticated" "log_grep" ">= 1" "$N" "[[ $N -ge 1 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 3: Write S0.3 — think→act loop**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S0.3"; MODULE="M1"; SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

TRACE="$ALEPH_TEST_EVIDENCE_DIR/trace.jsonl"
START=$(jq -r 'select(.event=="loop.start") | 1' "$TRACE" | wc -l | xargs)
END=$(jq -r   'select(.event=="loop.end")   | 1' "$TRACE" | wc -l | xargs)
THINK=$(jq -r 'select(.event=="think")       | 1' "$TRACE" | wc -l | xargs)
ACT=$(jq -r   'select(.event=="act")         | 1' "$TRACE" | wc -l | xargs)
check "loop_start"  "trace_assertion" ">= 1" "$START" "[[ $START -ge 1 ]]"
check "loop_end"    "trace_assertion" ">= 1" "$END"   "[[ $END   -ge 1 ]]"
check "think_event" "trace_assertion" ">= 1" "$THINK" "[[ $THINK -ge 1 ]]"
check "act_event"   "trace_assertion" ">= 1" "$ACT"   "[[ $ACT   -ge 1 ]]"

# session_events row count > 0
DB=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/data/state.db"
ROWS=$(sqlite3 "$DB" "SELECT COUNT(*) FROM session_events" 2>/dev/null || echo 0)
check "session_events_growth" "sql_assertion" ">= 1" "$ROWS" "[[ $ROWS -ge 1 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 4: Make executable + sanity check**

```bash
chmod +x scripts/validate-harness/S0.*.sh
for f in scripts/validate-harness/S0.*.sh; do bash -n "$f"; done
```

- [ ] **Step 5: Commit**

```bash
git add scripts/validate-harness/S0.*.sh
git commit -m "validate: S0 smoke prelude scripts (3)"
```

---

### Task 13: S1 — Coding Assistant (4 scripts)

**Files:**
- Create: `scripts/validate-harness/S1.1-tool-discovery.sh`
- Create: `scripts/validate-harness/S1.2-tool-calling-schema.sh`
- Create: `scripts/validate-harness/S1.3-sandbox-guardrail.sh`
- Create: `scripts/validate-harness/S1.4-stop-hook.sh`

- [ ] **Step 1: Write S1.1 — tool discovery (M2)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S1.1"; MODULE="M2"; SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

# operator pasted webchat response → response_log.md
RESP="$EVIDENCE_DIR/response_log.md"
TOOL_COUNT=$(grep -cE "^[ ]*[-*][ ]+\`?[a-z_]+\.[a-z_]+\`?" "$RESP" 2>/dev/null || echo 0)
check "tool_list_size" "response_assertion" ">= 30" "$TOOL_COUNT" "[[ $TOOL_COUNT -ge 30 ]]"

TRACE="$ALEPH_TEST_EVIDENCE_DIR/trace.jsonl"
LIST_HITS=$(jq -r 'select(.event=="tool_registry.list" or .tool_name=="tools.list") | 1' "$TRACE" | wc -l | xargs)
check "registry_list_event" "trace_assertion" ">= 1" "$LIST_HITS" "[[ $LIST_HITS -ge 1 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 2: Write S1.2 — tool calling schema (M6)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S1.2"; MODULE="M6"; SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

TRACE="$ALEPH_TEST_EVIDENCE_DIR/trace.jsonl"
VAL=$(jq -r 'select(.event=="tool_call.validated" and .valid==true) | 1' "$TRACE" | wc -l | xargs)
check "schemars_validation" "trace_assertion" ">= 1" "$VAL" "[[ $VAL -ge 1 ]]"

[[ -f /tmp/aleph-test/fib.rs ]] && FW=0 || FW=1
check "file_written" "fs_assertion" "exit 0" "$FW" "[[ $FW -eq 0 ]]"

ERR=$(jq -r 'select(.event=="tool_call.error" and .error_type=="tool_not_found") | 1' "$TRACE" | wc -l | xargs)
END=$(jq -r 'select(.event=="loop.end") | 1' "$TRACE" | wc -l | xargs)
check "tool_not_found_structured" "trace_assertion" ">= 1 + loop continued" "$ERR err / $END loop_end" \
      "[[ $ERR -ge 1 ]] && [[ $END -ge 1 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 3: Write S1.3 — sandbox guardrail (M9)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S1.3"; MODULE="M9"; SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

LOG=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/logs/aleph-server.log"
DEN=$(grep -c "sandbox\.denied" "$LOG" 2>/dev/null || echo 0)
check "sandbox_denied_event" "log_grep" ">= 1" "$DEN" "[[ $DEN -ge 1 ]]"

# / unchanged
EXISTS=$(test -d / && echo 1 || echo 0)
check "root_intact" "fs_assertion" "1" "$EXISTS" "[[ $EXISTS -eq 1 ]]"

# response contains denial reasoning
RESP="$EVIDENCE_DIR/response_log.md"
DEN_RESP=$(grep -ciE "denied|refused|sandbox|risk" "$RESP" 2>/dev/null || echo 0)
check "denial_in_response" "response_assertion" ">= 1" "$DEN_RESP" "[[ $DEN_RESP -ge 1 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 4: Write S1.4 — stop hook (M10)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S1.4"; MODULE="M10"; SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

LOG=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/logs/aleph-server.log"
FIRED=$(grep -c "stop_hook\.fired" "$LOG" 2>/dev/null || echo 0)
check "stop_hook_fired" "log_grep" ">= 1" "$FIRED" "[[ $FIRED -ge 1 ]]"

# session row stop_reason
DB=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/data/state.db"
HOOK_REASON=$(sqlite3 "$DB" "SELECT COUNT(*) FROM sessions WHERE stop_reason='hook_triggered'" 2>/dev/null || echo 0)
check "session_stop_reason" "sql_assertion" ">= 1" "$HOOK_REASON" "[[ $HOOK_REASON -ge 1 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 5: Sanity-check + commit**

```bash
chmod +x scripts/validate-harness/S1.*.sh
for f in scripts/validate-harness/S1.*.sh; do bash -n "$f"; done
git add scripts/validate-harness/S1.*.sh
git commit -m "validate: S1 coding-assistant scripts (4)"
```

---

### Task 14: S2 — Long Conversation Researcher (4 scripts)

**Files:**
- Create: `scripts/validate-harness/S2.1-prompt-assembly-layers.sh`
- Create: `scripts/validate-harness/S2.2-memory-write-recall.sh`
- Create: `scripts/validate-harness/S2.3-context-compaction.sh`
- Create: `scripts/validate-harness/S2.4-jit-retrieval.sh`

- [ ] **Step 1: Write S2.1 — prompt assembly layers (M5)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S2.1"; MODULE="M5"; SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

TRACE="$ALEPH_TEST_EVIDENCE_DIR/trace.jsonl"
SECTIONS=$(jq -r 'select(.event=="prompt_assembled") | .sections | length' "$TRACE" | sort -nu | tail -1)
SECTIONS=${SECTIONS:-0}
check "section_count" "trace_assertion" ">= 5" "$SECTIONS" "[[ $SECTIONS -ge 5 ]]"

# Has all 5 expected sections
HAS=$(jq -r 'select(.event=="prompt_assembled") | .sections | join(",")' "$TRACE" | tail -1)
for layer in system tools memory history user; do
  H=$(echo "$HAS" | grep -c "$layer" || echo 0)
  check "section_${layer}" "trace_assertion" "present" "$H" "[[ $H -ge 1 ]]"
done

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 2: Write S2.2 — memory write+recall (M3)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S2.2"; MODULE="M3"; SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

DB=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/data/state.db"
WRITES=$(sqlite3 "$DB" "SELECT COUNT(*) FROM memory_events WHERE event_type='memory_write'" 2>/dev/null || echo 0)
check "memory_write_event" "sql_assertion" ">= 1" "$WRITES" "[[ $WRITES -ge 1 ]]"

# Hermes-9 codename present in any prompt_assembled memory section after turn 1
TRACE="$ALEPH_TEST_EVIDENCE_DIR/trace.jsonl"
HERMES=$(jq -r 'select(.event=="prompt_assembled") | .memory_section_text // ""' "$TRACE" | grep -c "Hermes-9" || echo 0)
check "memory_in_assembled_prompt" "trace_assertion" ">= 1" "$HERMES" "[[ $HERMES -ge 1 ]]"

# operator-recorded recall
RESP="$EVIDENCE_DIR/response_log.md"
RECALL=$(grep -c "Hermes-9" "$RESP" 2>/dev/null || echo 0)
check "llm_recall_codename" "response_assertion" ">= 1" "$RECALL" "[[ $RECALL -ge 1 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 3: Write S2.3 — context compaction (M4)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S2.3"; MODULE="M4"; SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

LOG=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/logs/aleph-server.log"
TRIG=$(grep -c "compaction_triggered" "$LOG" 2>/dev/null || echo 0)
check "compaction_triggered" "log_grep" ">= 1" "$TRIG" "[[ $TRIG -ge 1 ]]"

PRESS=$(grep -c "pressure_level=high\|pressure_level\":\"high" "$LOG" 2>/dev/null || echo 0)
check "pressure_high" "log_grep" ">= 1" "$PRESS" "[[ $PRESS -ge 1 ]]"

TRACE="$ALEPH_TEST_EVIDENCE_DIR/trace.jsonl"
STRAT=$(jq -r 'select(.event=="compactor.strategy_chosen") | 1' "$TRACE" | wc -l | xargs)
check "strategy_chosen" "trace_assertion" ">= 1" "$STRAT" "[[ $STRAT -ge 1 ]]"

# Token delta: pre vs post compaction prompt size ≥ 30% reduction
PRE=$(jq -r 'select(.event=="prompt_assembled") | .total_tokens // 0' "$TRACE" | sort -nu | tail -1)
POST=$(jq -r 'select(.event=="prompt_assembled" and .post_compaction==true) | .total_tokens // 0' "$TRACE" | sort -n | head -1)
PRE=${PRE:-0}; POST=${POST:-0}
RATIO=0
[[ "$PRE" -gt 0 ]] && RATIO=$(( (PRE - POST) * 100 / PRE ))
check "compaction_reduction_pct" "trace_assertion" ">= 30" "${RATIO}" "[[ $RATIO -ge 30 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 4: Write S2.4 — JIT retrieval (M3+M4)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S2.4"; MODULE="M3"; SEVERITY="medium"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

TRACE="$ALEPH_TEST_EVIDENCE_DIR/trace.jsonl"
RET=$(jq -r 'select(.event=="memory.retrieve") | 1' "$TRACE" | wc -l | xargs)
check "memory_retrieve_event" "trace_assertion" ">= 1" "$RET" "[[ $RET -ge 1 ]]"

CHUNKS=$(jq -r 'select(.event=="memory.retrieve") | .chunk_ids | length' "$TRACE" | sort -nu | tail -1)
CHUNKS=${CHUNKS:-0}
check "chunks_retrieved" "trace_assertion" ">= 1" "$CHUNKS" "[[ $CHUNKS -ge 1 ]]"

# operator notes whether LLM correctly answered with the recalled fact
RESP="$EVIDENCE_DIR/response_log.md"
HIT=$(grep -ciE "author|paper|first author" "$RESP" 2>/dev/null || echo 0)
check "llm_answered_with_fact" "response_assertion" ">= 1" "$HIT" "[[ $HIT -ge 1 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 5: Sanity-check + commit**

```bash
chmod +x scripts/validate-harness/S2.*.sh
for f in scripts/validate-harness/S2.*.sh; do bash -n "$f"; done
git add scripts/validate-harness/S2.*.sh
git commit -m "validate: S2 long-conversation scripts (4)"
```

---

### Task 15: S3 — Multi-Agent Coordination (3 scripts)

**Files:**
- Create: `scripts/validate-harness/S3.1-subagent-fork.sh`
- Create: `scripts/validate-harness/S3.2-checkpoint-replay.sh`
- Create: `scripts/validate-harness/S3.3-error-handling-4class.sh`

- [ ] **Step 1: Write S3.1 — subagent fork (M11)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S3.1"; MODULE="M11"; SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

DB=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/data/state.db"
SPAWN=$(sqlite3 "$DB" "SELECT COUNT(*) FROM agent_events WHERE event_type='subagent_spawned'" 2>/dev/null || echo 0)
check "spawn_event" "sql_assertion" ">= 1" "$SPAWN" "[[ $SPAWN -ge 1 ]]"

NEW_SESS=$(sqlite3 "$DB" "SELECT COUNT(*) FROM sessions WHERE parent_session_id IS NOT NULL" 2>/dev/null || echo 0)
check "child_session_row" "sql_assertion" ">= 1" "$NEW_SESS" "[[ $NEW_SESS -ge 1 ]]"

TRACE="$ALEPH_TEST_EVIDENCE_DIR/trace.jsonl"
HANDOFF=$(jq -r 'select(.event=="subagent.handoff_back") | 1' "$TRACE" | wc -l | xargs)
check "handoff_back" "trace_assertion" ">= 1" "$HANDOFF" "[[ $HANDOFF -ge 1 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 2: Write S3.2 — checkpoint replay (M7)**

This script also performs the side-channel REST or binary call (per Phase A finding #3).

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S3.2"; MODULE="M7"; SEVERITY="medium"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

# Identify a session to replay (the one where main thread happened most recently)
DB=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/data/state.db"
SID=$(sqlite3 "$DB" "SELECT session_id FROM sessions ORDER BY started_at DESC LIMIT 1")
[[ -z "$SID" ]] && fail "no session to replay"
HEAD_BEFORE=$(sqlite3 "$DB" "SELECT MAX(seq) FROM session_events WHERE session_id='$SID'")

# Branch on Phase A finding #3
if [[ -f /tmp/aleph-replay-strategy.txt ]]; then
  STRATEGY=$(cat /tmp/aleph-replay-strategy.txt)
else
  STRATEGY="curl"
fi

case "$STRATEGY" in
  curl)
    RESP=$(curl -sf -X POST -H "Authorization: Bearer aleph-9976129a-407d-4893-a96c-6467b24bedac" \
                 "http://127.0.0.1:9090/api/sessions/$SID/replay?from_seq=3&to_seq=8")
    HASH=$(echo "$RESP" | jq -r .state_hash)
    HEAD_AFTER=$(echo "$RESP" | jq -r .head_seq)
    ;;
  binary)
    RESP=$(./target/release/aleph-replay-probe --session-id "$SID" --from 3 --to 8)
    HASH=$(echo "$RESP" | jq -r .state_hash)
    HEAD_AFTER=$(echo "$RESP" | jq -r .head_seq)
    ;;
esac

check "replay_succeeded" "http_assertion" "non-empty hash" "${HASH:-empty}" "[[ -n \"$HASH\" ]]"
check "head_seq_match"   "sql_assertion"  "8" "$HEAD_AFTER" "[[ \"$HEAD_AFTER\" == \"8\" ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 3: Write S3.3 — error handling 4-class (M8)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S3.3"; MODULE="M8"; SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

TRACE="$ALEPH_TEST_EVIDENCE_DIR/trace.jsonl"
LOG=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/logs/aleph-server.log"

# (a) transient
A=$(jq -r 'select(.event=="error.transient") | 1' "$TRACE" | wc -l | xargs)
check "error_transient"      "trace_assertion" ">= 1" "$A" "[[ $A -ge 1 ]]"
RETRIES=$(jq -r 'select(.event=="provider.retry") | 1' "$TRACE" | wc -l | xargs)
check "auto_retry_attempted" "trace_assertion" ">= 1" "$RETRIES" "[[ $RETRIES -ge 1 ]]"

# (b) recoverable
B=$(jq -r 'select(.event=="error.recoverable") | 1' "$TRACE" | wc -l | xargs)
check "error_recoverable"    "trace_assertion" ">= 1" "$B" "[[ $B -ge 1 ]]"

# (c) user_fixable
C=$(jq -r 'select(.event=="error.user_fixable") | 1' "$TRACE" | wc -l | xargs)
check "error_user_fixable"   "trace_assertion" ">= 1" "$C" "[[ $C -ge 1 ]]"
APPROVAL=$(grep -c "approval\.requested" "$LOG" 2>/dev/null || echo 0)
check "approval_prompted"    "log_grep" ">= 1" "$APPROVAL" "[[ $APPROVAL -ge 1 ]]"

# (d) unexpected + supervisor restart
D=$(jq -r 'select(.event=="error.unexpected") | 1' "$TRACE" | wc -l | xargs)
check "error_unexpected"     "trace_assertion" ">= 1" "$D" "[[ $D -ge 1 ]]"
RESTART=$(grep -c "supervisor.*restart\|process_supervisor.*restart" "$LOG" 2>/dev/null || echo 0)
check "supervisor_restarted" "log_grep" ">= 1" "$RESTART" "[[ $RESTART -ge 1 ]]"

# main session not crashed
END=$(jq -r 'select(.event=="loop.end") | 1' "$TRACE" | wc -l | xargs)
check "main_loop_continued"  "trace_assertion" ">= 4" "$END" "[[ $END -ge 4 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 4: Sanity-check + commit**

```bash
chmod +x scripts/validate-harness/S3.*.sh
for f in scripts/validate-harness/S3.*.sh; do bash -n "$f"; done
git add scripts/validate-harness/S3.*.sh
git commit -m "validate: S3 multi-agent scripts (3)"
```

---

### Task 16: S4 — Daily Assistant + Configuration (4 scripts)

**Files:**
- Create: `scripts/validate-harness/S4.1-boot-deep.sh`
- Create: `scripts/validate-harness/S4.2-pii-guardrail.sh`
- Create: `scripts/validate-harness/S4.3-tool-as-config.sh`
- Create: `scripts/validate-harness/S4.4-skill-prefetch.sh`

- [ ] **Step 1: Write S4.1 — boot deep (M12)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S4.1"; MODULE="M12"; SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

BOOT="$ALEPH_TEST_EVIDENCE_DIR/boot.log"
SEQ=$(grep "boot\.module\.init" "$BOOT" | awk '{print $NF}' | tr '\n' ',')
check "boot_init_sequence_recorded" "log_grep" "non-empty" "$SEQ" "[[ -n \"$SEQ\" ]]"

# /health subsystems object has ≥ 12 keys
HEALTH_JSON=$(curl -sf http://127.0.0.1:9090/health)
KEYS=$(echo "$HEALTH_JSON" | jq -r '.subsystems // {} | keys | length')
KEYS=${KEYS:-0}
check "health_subsystem_count" "http_assertion" ">= 12" "$KEYS" "[[ $KEYS -ge 12 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 2: Write S4.2 — PII guardrail (M9-PII)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S4.2"; MODULE="M9"; SEVERITY="critical"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

LOG=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/logs/aleph-server.log"
DET=$(grep -c "pii\.detected" "$LOG" 2>/dev/null || echo 0)
check "pii_detected" "log_grep" ">= 2" "$DET" "[[ $DET -ge 2 ]]"

RED=$(grep -c "pii\.redacted" "$LOG" 2>/dev/null || echo 0)
check "pii_redacted" "log_grep" ">= 1" "$RED" "[[ $RED -ge 1 ]]"

# response text contains no raw PII
RESP="$EVIDENCE_DIR/response_log.md"
ID_HITS=$(grep -cE "[1-9][0-9]{16}[0-9X]" "$RESP" 2>/dev/null || echo 0)
EMAIL_HITS=$(grep -cE "user@example\.com" "$RESP" 2>/dev/null || echo 0)
check "no_raw_id_in_response"    "response_assertion" "0" "$ID_HITS" "[[ $ID_HITS -eq 0 ]]"
check "no_raw_email_in_response" "response_assertion" "0" "$EMAIL_HITS" "[[ $EMAIL_HITS -eq 0 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 3: Write S4.3 — tool-as-config (R9 + M2)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S4.3"; MODULE="M2"; SEVERITY="high"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

TRACE="$ALEPH_TEST_EVIDENCE_DIR/trace.jsonl"
CALL=$(jq -r 'select(.event=="tool_call.dispatched" and (.tool_name=="channel.create" or .tool_name=="channels.create")) | 1' "$TRACE" | wc -l | xargs)
check "channel_create_tool_called" "trace_assertion" ">= 1" "$CALL" "[[ $CALL -ge 1 ]]"

CFG=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/data/config.toml"
HAS_CHAN=$(grep -c "\[channels\.\|@aleph_news\|fake-bot-token" "$CFG" 2>/dev/null || echo 0)
check "config_toml_updated" "fs_assertion" ">= 1" "$HAS_CHAN" "[[ $HAS_CHAN -ge 1 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 4: Write S4.4 — skill prefetch (cross-cut)**

```bash
#!/usr/bin/env bash
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/_lib.sh"
SCN="S4.4"; MODULE="skill_prefetch"; SEVERITY="medium"
SCN_STARTED_AT=$(date -u +%FT%TZ)
EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR/$SCN"
mkdir -p "$EVIDENCE_DIR"

LOG=$(jq -r .test_home "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json")"/.aleph/logs/aleph-server.log"
PRE=$(grep -c "skill\.prefetch\.completed" "$LOG" 2>/dev/null || echo 0)
check "prefetch_completed" "log_grep" ">= 1" "$PRE" "[[ $PRE -ge 1 ]]"

CNT=$(grep "skill\.prefetch\.completed" "$LOG" 2>/dev/null | grep -oE "skill_count=[0-9]+" | head -1 | cut -d= -f2)
CNT=${CNT:-0}
check "prefetch_skill_count" "log_grep" ">= 1" "$CNT" "[[ $CNT -ge 1 ]]"

# operator-recorded slash menu count
RESP="$EVIDENCE_DIR/response_log.md"
SLASH=$(grep -cE "^[ ]*/[a-z_-]+" "$RESP" 2>/dev/null || echo 0)
check "slash_menu_rendered" "response_assertion" ">= 1" "$SLASH" "[[ $SLASH -ge 1 ]]"

emit_evidence "$SCN" "$MODULE" "$SEVERITY"
```

- [ ] **Step 5: Sanity-check + commit**

```bash
chmod +x scripts/validate-harness/S4.*.sh
for f in scripts/validate-harness/S4.*.sh; do bash -n "$f"; done
git add scripts/validate-harness/S4.*.sh
git commit -m "validate: S4 daily-assistant scripts (4)"
```

---

## Phase D — Fixtures & Dry Run (2 tasks)

### Task 17: Prepare 10 long-form text fixtures for S2.3

**Files:**
- Create: `scripts/validate-harness/fixtures/long-text-01.md` … `long-text-10.md`

- [ ] **Step 1: Generate 10 fixtures**

Use 10 ~3000-token chunks of synthetic research-paper-like prose. Avoid copyrighted material. Each file has a distinct `paper_id`, `author`, `key_finding` so S2.4 can later ask "what was paper 5's first author".

```bash
mkdir -p scripts/validate-harness/fixtures
# Use a one-shot LLM call OR copy 10 sections from project's own docs/reference/ as substrate
# Each file ~3000 tokens, marked with a unique fact like:
#   --- paper_id: P5 ---
#   --- author: Dr. Synthetic Five ---
#   ...
```

A workable shortcut: use 10 files from `docs/reference/` (ARCHITECTURE, AGENT_SYSTEM, GATEWAY, etc.), each prefixed with a synthetic header line containing a unique `author` token. The substantive content is real Aleph docs; the synthetic header is the recall target.

- [ ] **Step 2: Verify total token mass**

```bash
wc -w scripts/validate-harness/fixtures/long-text-*.md | tail -1
```

Expected: ≥ 30,000 words total (rough proxy for ≥ 30k tokens).

- [ ] **Step 3: Commit**

```bash
git add scripts/validate-harness/fixtures/
git commit -m "validate: 10 long-text fixtures for S2.3"
```

---

### Task 18: Dry run with synthetic evidence

**Files:**
- Create: `scripts/validate-harness/dry-run.sh`

- [ ] **Step 1: Write a dry-run harness**

This script generates synthetic `trace.jsonl`, server.log, and SQLite content matching the assertions in every scenario script, runs each script, and verifies all 18 emit `evidence.json` with `status=pass`. This is the test-of-the-test.

```bash
#!/usr/bin/env bash
# Dry run: synthesize evidence files that satisfy every check, run every script.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d /tmp/dry-run-XXXX)"
export ALEPH_TEST_EVIDENCE_DIR="$TMP"
mkdir -p "$TMP"

# Synthetic run-meta
mkdir -p "$TMP/test-home/.aleph/data" "$TMP/test-home/.aleph/logs"
sqlite3 "$TMP/test-home/.aleph/data/state.db" <<EOF
CREATE TABLE session_events (id INTEGER, session_id TEXT, seq INTEGER);
INSERT INTO session_events VALUES (1, 'sess-1', 1), (2, 'sess-1', 2), (3, 'sess-1', 3),
  (4, 'sess-1', 4), (5, 'sess-1', 5), (6, 'sess-1', 6), (7, 'sess-1', 7), (8, 'sess-1', 8);
CREATE TABLE memory_events (event_type TEXT);
INSERT INTO memory_events VALUES ('memory_write');
CREATE TABLE agent_events (event_type TEXT);
INSERT INTO agent_events VALUES ('subagent_spawned');
CREATE TABLE sessions (session_id TEXT, parent_session_id TEXT, started_at TEXT, stop_reason TEXT);
INSERT INTO sessions VALUES ('sess-1', NULL, '2026-04-25', 'hook_triggered');
INSERT INTO sessions VALUES ('sess-2', 'sess-1', '2026-04-25', NULL);
EOF

cat > "$TMP/run-meta.json" <<EOF
{"test_home":"$TMP/test-home","started_at":"2026-04-25T15:00:00Z",
 "build_commit":"DRY","aleph_version":"DRY","aleph_pid":0}
EOF

# Synthetic trace.jsonl with every required event
cat > "$TMP/trace.jsonl" <<'EOF'
{"event":"loop.start"}
{"event":"think"}
{"event":"act"}
{"event":"loop.end"}
{"event":"loop.end"}
{"event":"loop.end"}
{"event":"loop.end"}
{"event":"tool_registry.list"}
{"event":"tool_call.validated","valid":true}
{"event":"tool_call.error","error_type":"tool_not_found"}
{"event":"prompt_assembled","sections":["system","tools","memory","history","user"],"total_tokens":10000,"memory_section_text":"Hermes-9 ..."}
{"event":"prompt_assembled","sections":["system","tools","memory","history","user"],"post_compaction":true,"total_tokens":3000}
{"event":"compactor.strategy_chosen","strategy":"summarize"}
{"event":"memory.retrieve","chunk_ids":["c1"]}
{"event":"subagent.handoff_back"}
{"event":"error.transient"}
{"event":"error.recoverable"}
{"event":"error.user_fixable"}
{"event":"error.unexpected"}
{"event":"provider.retry"}
{"event":"tool_call.dispatched","tool_name":"channel.create"}
EOF

# Synthetic server.log with every grep pattern
cat > "$TMP/test-home/.aleph/logs/aleph-server.log" <<'EOF'
gateway.ws.authenticated token=aleph-9976...bedac
sandbox.denied risk_level=high
stop_hook.fired hook_id=h1
compaction_triggered pressure_level=high
approval.requested id=a1
process_supervisor restart child=c1
pii.detected
pii.detected
pii.redacted
skill.prefetch.completed skill_count=12
boot.module.init phase=tools
boot.module.init phase=memory
boot.module.init phase=context
boot.module.init phase=prompt
boot.module.init phase=verification
boot.module.init phase=guardrails
boot.module.init phase=subagent
boot.module.init phase=state
boot.module.init phase=error
boot.module.init phase=tool_call
boot.module.init phase=orchestration
boot.module.init phase=init
boot.complete
EOF
cp "$TMP/test-home/.aleph/logs/aleph-server.log" "$TMP/boot.log"

# Synthetic config.toml with channel
cat > "$TMP/test-home/.aleph/data/config.toml" <<'EOF'
[channels.tg-news]
bot_token = "fake-bot-token-xyz"
channel = "@aleph_news"
EOF

# Synthetic per-scenario response_log.md (operator-style)
for scn in S1.1 S1.3 S2.2 S2.4 S4.2 S4.4; do
  mkdir -p "$TMP/$scn"
done
# S1.1: 30+ tools list
{
  for i in $(seq 1 35); do echo "- \`fs.tool_$i\`"; done
} > "$TMP/S1.1/response_log.md"
# S1.3: denial
echo "Denied. Sandbox refused due to risk_level=high." > "$TMP/S1.3/response_log.md"
# S2.2: codename recall
echo "Hermes-9" > "$TMP/S2.2/response_log.md"
# S2.4: paper author
echo "first author: Dr. Synthetic Five" > "$TMP/S2.4/response_log.md"
# S4.2: PII redacted
echo "Dear customer service, my contact has been redacted ..." > "$TMP/S4.2/response_log.md"
# S4.4: slash menu
echo "/skill-1" > "$TMP/S4.4/response_log.md"

# /tmp/aleph-test/fib.rs presence for S1.2
mkdir -p /tmp/aleph-test && touch /tmp/aleph-test/fib.rs

# Stand up a minimal /health server (python one-liner)
python3 -c "
import http.server, socketserver, json, threading
class H(http.server.BaseHTTPRequestHandler):
  def do_GET(s):
    s.send_response(200); s.send_header('content-type','application/json'); s.end_headers()
    s.wfile.write(json.dumps({'subsystems':{f'm{i}':'ok' for i in range(12)}}).encode())
  def log_message(s,*a): pass
srv = socketserver.TCPServer(('127.0.0.1',9090),H); threading.Thread(target=srv.serve_forever,daemon=True).start()
import time; time.sleep(120)
" &
HPID=$!
sleep 1

# Run every scenario script
PASS=0 FAIL=0
for scn in S0.1 S0.2 S0.3 S1.1 S1.2 S1.3 S1.4 S2.1 S2.2 S2.3 S2.4 S3.1 S3.2 S3.3 S4.1 S4.2 S4.3 S4.4; do
  fname=$(ls "$SCRIPT_DIR"/${scn}-*.sh 2>/dev/null | head -1)
  if [[ -z "$fname" ]]; then echo "SKIP $scn (no script)"; continue; fi
  if bash "$fname" 2>/dev/null; then
    echo "✅ $scn"; PASS=$((PASS+1))
  else
    rc=$?
    STATUS=$(jq -r .status "$TMP/$scn/evidence.json" 2>/dev/null || echo "no-evidence")
    echo "❌ $scn (rc=$rc, status=$STATUS)"; FAIL=$((FAIL+1))
  fi
done

kill $HPID 2>/dev/null || true
echo
echo "DRY RUN: $PASS passed, $FAIL failed"
[[ $FAIL -eq 0 ]] || exit 1
```

- [ ] **Step 2: Run dry run**

```bash
chmod +x scripts/validate-harness/dry-run.sh
bash scripts/validate-harness/dry-run.sh
```

Expected: `DRY RUN: 18 passed, 0 failed`. If any fail, fix the corresponding scenario script's check expressions until the synthetic fixture passes (this catches typos and quoting bugs in `_lib.sh::check`).

- [ ] **Step 3: Commit**

```bash
git add scripts/validate-harness/dry-run.sh
git commit -m "validate: dry-run harness (18 synthetic-pass scenarios)"
```

---

## Phase E — Live Validation Run (1 task)

### Task 19: Execute the validation against live Aleph

**Files:**
- None created (this is execution).

- [ ] **Step 1: Pre-flight**

```bash
bash scripts/validate-harness/preflight.sh
```

Expected: `==> health OK after Ns` and operator instructions. If pre-flight fails (port collision, build error, health timeout), fix and re-run.

- [ ] **Step 2: Open browser webchat**

```
http://127.0.0.1:8080/?token=aleph-9976129a-407d-4893-a96c-6467b24bedac
```

Confirm the WS handshake succeeds (chat input is enabled, no error toast).

- [ ] **Step 3: Run scenarios via run-all.sh in conductor mode**

```bash
ALEPH_TEST_EVIDENCE_DIR="$PWD/target/test-evidence" \
  bash scripts/validate-harness/run-all.sh
```

The conductor (Claude) reads each prompt from spec §5 to the operator (user); operator pastes into webchat, captures the response into `target/test-evidence/<scn>/response_log.md`, presses Enter; the orchestrator runs the validation script and reports pass/fail.

For S2.3, paste the 10 fixtures from Phase D in sequence.

For S3.2 and S3.3 a/d, the conductor performs side-channel actions (curl, kill -9) directly without operator input.

- [ ] **Step 4: Post-flight**

```bash
bash scripts/validate-harness/postflight.sh
```

Reads `REPORT.md` aloud:

```bash
cat target/test-evidence/REPORT.md
```

- [ ] **Step 5: Decision based on report**

- If `Critical fails: 0` and `High fails: 0` and 12-module matrix all green: validation **PASSED**. Commit the report (it's git-ignored under `target/`, so optionally copy to `docs/superpowers/reports/2026-04-25-validation-report.md`).
- If any Critical fail: investigate root cause; may not be a refactor regression but a test-rig bug. Re-run only the affected scenario after fix.
- If only Medium/Low fails: file follow-up issues; validation still PASSED.

- [ ] **Step 6: (Optional) Save report into docs**

```bash
cp target/test-evidence/REPORT.md docs/superpowers/reports/$(date +%F)-validation-report.md
git add docs/superpowers/reports/
git commit -m "validate: production validation report $(date +%F)"
```

---

## Spec Coverage Cross-Check (Self-Review)

| Spec section | Covered by tasks |
|---|---|
| §3 Architecture (dual-channel, HOME redirect) | T8 (preflight wires HOME + trace env) |
| §4 12-module ontology | T12–T16 (one task per session, all 12 modules + 2 cross-cutting hit) |
| §5.1–§5.5 18 sub-scenarios | T12 (S0×3) + T13 (S1×4) + T14 (S2×4) + T15 (S3×3) + T16 (S4×4) = 18 ✓ |
| §5.6 module matrix | T11 render-report.py renders the matrix from evidence.json |
| §6.1 evidence.json schema | T7 _lib.sh::emit_evidence emits exact schema |
| §6.2 6 check kinds | T12–T16 use all 6 across scenarios |
| §6.3 script skeleton | T7 _lib.sh provides; T12–T16 instantiate |
| §6.4 severity tier policy | T7 emit_evidence records `severity_on_fail`; T10 run-all.sh enforces tier branching |
| §6.5 REPORT.md format | T11 render-report.py produces |
| §7.1 pre-flight 10 steps | T8 all 10 steps present |
| §7.2 conductor mode | T19 step 3 |
| §7.3 failure escalation | T10 explicit branching on severity + exit code |
| §7.4 post-flight 6 steps | T9 all 6 steps present |
| §8 Verification-Required (6 items) | T1–T6 (one task per TBD, fallback documented) |
| §9 file layout | T7–T18 produce exactly the listed files |
| §10 Out of Scope | None of the YAGNI items appear in any task |

All 18 sub-scenarios accounted for. All 6 TBDs accounted for. No placeholders in plan. Severity assignments match spec §6.4. Type consistency: every script uses `_lib.sh`'s `check` / `emit_evidence` exactly as defined in T7.

---

## Plan complete and saved to `docs/superpowers/plans/2026-04-25-harness-production-validation.md`.

**Two execution options:**

1. **Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration. Best for the 6 Phase A probes (each is short and independent) and for the 5 scenario-script tasks (mechanical pattern, easy review).

2. **Inline Execution** — Execute tasks in this session using executing-plans, batch with checkpoints. Best for the live run (T19) which requires real-time conductor coordination.

Suggestion: **Subagent-Driven for Phase A–D (T1–T18)**, then **Inline for Phase E (T19)** because the live validation requires me + you in the same session.

**Which approach?**
