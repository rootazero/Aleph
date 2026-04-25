#!/usr/bin/env bash
# Pre-flight: kill aleph, build, set up isolated HOME, start aleph, wait for /health.
# Spec: docs/superpowers/specs/2026-04-25-harness-production-validation-design.md §7.1
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# ----------------------------------------------------------------------------
# Step 1: Kill existing aleph processes (CLAUDE.md redline — vault corruption)
# ----------------------------------------------------------------------------
echo "==> [1/10] killing any existing aleph-server processes"
pkill -f "target/release/aleph-server" 2>/dev/null || true
pkill -f "target/debug/aleph-server"   2>/dev/null || true
sleep 2
if ps aux | grep "[a]leph-server" | grep -v zsh | grep -v cp | grep -v tail >/dev/null; then
  echo "FATAL: aleph processes still alive; refuse to continue" >&2
  ps aux | grep "[a]leph-server" | grep -v zsh >&2
  exit 1
fi

# ----------------------------------------------------------------------------
# Step 2: Port collision check (18790 = aleph gateway + webchat panel)
# ----------------------------------------------------------------------------
echo "==> [2/10] checking port 18790"
for port in 18790; do
  if lsof -i:"$port" >/dev/null 2>&1; then
    echo "FATAL: port $port is in use" >&2
    lsof -i:"$port" >&2
    exit 1
  fi
done

# ----------------------------------------------------------------------------
# Step 3: Release build (~10 min)
# ----------------------------------------------------------------------------
echo "==> [3/10] just build"
just build

# ----------------------------------------------------------------------------
# Step 4: Set up isolated test HOME
# ----------------------------------------------------------------------------
echo "==> [4/10] setting up isolated test HOME"
export ALEPH_TEST_HOME="/tmp/aleph-validate-$(date +%s)"
mkdir -p "$ALEPH_TEST_HOME/.aleph/data" "$ALEPH_TEST_HOME/.aleph/logs"
export ALEPH_TEST_EVIDENCE_DIR="$REPO_ROOT/target/test-evidence"
mkdir -p "$ALEPH_TEST_EVIDENCE_DIR"
echo "    ALEPH_TEST_HOME=$ALEPH_TEST_HOME"
echo "    ALEPH_TEST_EVIDENCE_DIR=$ALEPH_TEST_EVIDENCE_DIR"

# Cleanup trap: on early abort (non-zero exit), kill aleph and remove test HOME.
# Disarmed at the end of step 10 so a successful run leaves the test HOME intact.
cleanup() {
  local rc=$?
  if [[ "$rc" -ne 0 ]]; then
    echo "==> aborting (rc=$rc); cleaning up" >&2
    if [[ -n "${ALEPH_PID:-}" ]]; then
      kill -TERM "$ALEPH_PID" 2>/dev/null || true
      sleep 1
      kill -KILL "$ALEPH_PID" 2>/dev/null || true
    fi
    if [[ -n "${ALEPH_TEST_HOME:-}" && -d "$ALEPH_TEST_HOME" ]]; then
      rm -rf "$ALEPH_TEST_HOME"
      echo "==> removed $ALEPH_TEST_HOME" >&2
    fi
  fi
}
trap cleanup EXIT INT TERM

# ----------------------------------------------------------------------------
# Step 5: Copy real → test (vault, providers, skills config, agents)
# ----------------------------------------------------------------------------
echo "==> [5/10] copying real config into test HOME"
SRC_HOME="$HOME/.aleph"
DST_HOME="$ALEPH_TEST_HOME/.aleph"
# Parent-level: config.toml, agents/, channels/, guides/ (per real ~/.aleph layout)
for item in config.toml agents channels guides; do
  if [[ -e "$SRC_HOME/$item" ]]; then
    cp -R "$SRC_HOME/$item" "$DST_HOME/"
  fi
done
# Inner data/: only vault + shared token; DBs start fresh for clean evidence
for item in secrets.vault .shared_token; do
  if [[ -e "$SRC_HOME/data/$item" ]]; then
    cp "$SRC_HOME/data/$item" "$DST_HOME/data/"
  fi
done
PARENT_N=$(ls "$DST_HOME" 2>/dev/null | wc -l | xargs)
DATA_N=$(ls "$DST_HOME/data" 2>/dev/null | wc -l | xargs)
echo "    copied $PARENT_N parent items + $DATA_N data items into test HOME"

# ----------------------------------------------------------------------------
# Step 6: Start aleph with HOME redirect + RUST_LOG
# ----------------------------------------------------------------------------
echo "==> [6/10] starting aleph-server"
LOG="$ALEPH_TEST_HOME/.aleph/logs/aleph-server.log"
HOME="$ALEPH_TEST_HOME" \
  RUST_LOG="alephcore::harness=trace,alephcore::orchestrator=trace,alephcore=info" \
  ./target/release/aleph-server start \
  > "$LOG" 2>&1 &
ALEPH_PID=$!
echo "$ALEPH_PID" > "$ALEPH_TEST_EVIDENCE_DIR/.aleph.pid"
echo "    aleph started (pid=$ALEPH_PID, log=$LOG)"

# ----------------------------------------------------------------------------
# Step 7: Wait for /health 200 (90s timeout)
# ----------------------------------------------------------------------------
echo "==> [7/10] waiting for /health (90s timeout)"
HEALTH_OK=0
for i in $(seq 1 90); do
  if curl -sf http://127.0.0.1:18790/health >/dev/null 2>&1; then
    echo "==> health OK after ${i}s"
    HEALTH_OK=1
    break
  fi
  sleep 1
done
if [[ "$HEALTH_OK" -ne 1 ]]; then
  echo "FATAL: /health timeout after 90s" >&2
  sleep 0.5  # allow log buffer to flush
  tail -50 "$LOG" >&2
  exit 1   # the EXIT trap (added in step 4) will kill aleph + clean test HOME
fi

# ----------------------------------------------------------------------------
# Step 8: Extract boot init events for S0.1 / S4.1
# ----------------------------------------------------------------------------
echo "==> [8/10] extracting boot events"
grep -E "boot\.module\.init|boot\.complete|boot\.phase" "$LOG" \
  > "$ALEPH_TEST_EVIDENCE_DIR/boot.log" || true
echo "    boot events → $ALEPH_TEST_EVIDENCE_DIR/boot.log ($(wc -l < "$ALEPH_TEST_EVIDENCE_DIR/boot.log" | xargs) lines)"

# ----------------------------------------------------------------------------
# Step 9: Write run-meta.json
# ----------------------------------------------------------------------------
echo "==> [9/10] writing run-meta.json"
ALEPH_VERSION=$(< VERSION)
ALEPH_VERSION="${ALEPH_VERSION%$'\n'}"
cat > "$ALEPH_TEST_EVIDENCE_DIR/run-meta.json" <<EOF
{
  "started_at": "$(date -u +%FT%TZ)",
  "build_commit": "$(git rev-parse HEAD)",
  "aleph_version": "$ALEPH_VERSION",
  "test_home": "$ALEPH_TEST_HOME",
  "evidence_dir": "$ALEPH_TEST_EVIDENCE_DIR",
  "gateway_token_prefix": "aleph-9976",
  "aleph_pid": $ALEPH_PID,
  "rust_log": "alephcore::harness=trace,alephcore::orchestrator=trace,alephcore=info"
}
EOF

# ----------------------------------------------------------------------------
# Step 10: Print operator instructions
# ----------------------------------------------------------------------------
echo "==> [10/10] preflight complete"
trap - EXIT INT TERM   # disarm: success-path leaves test HOME for the operator
cat <<EOF

================================================================================
  PRE-FLIGHT COMPLETE — aleph-server running in isolated test HOME
================================================================================

  Test HOME      : $ALEPH_TEST_HOME
  Evidence dir   : $ALEPH_TEST_EVIDENCE_DIR
  Server PID     : $ALEPH_PID
  Server log     : $LOG

  Webchat URL (open in browser):
    http://127.0.0.1:18790/?token=aleph-9976129a-407d-4893-a96c-6467b24bedac

  Next step — run the scenario suite:
    scripts/validate-harness/run-all.sh

  If you spawn a new shell and need these env vars, source this block:
    export ALEPH_TEST_HOME="$ALEPH_TEST_HOME"
    export ALEPH_TEST_EVIDENCE_DIR="$ALEPH_TEST_EVIDENCE_DIR"

  Note: re-running preflight overwrites prior target/test-evidence/ contents.

  Tear-down (when done):
    kill \$(cat "$ALEPH_TEST_EVIDENCE_DIR/.aleph.pid")
    rm -rf "$ALEPH_TEST_HOME"

================================================================================
EOF
