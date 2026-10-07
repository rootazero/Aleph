#!/usr/bin/env bash
# Real-binary H-pre QA. This fixture deliberately reports unsupported controls as
# UNVERIFIED; it never adds a diagnostic/admin route to make them drivable.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
GUARD="$REPO/.superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py"
BUSY="$REPO/qa/busy_input"
PATCH="$REPO/qa/busy_input/patch_config.py"
MCP="$REPO/qa/plugins/mcp_mock_server.py"
PYTHON="${PYTHON:-$(command -v python3)}"
GATEWAY_PORT="${GATEWAY_PORT:-18831}"
MOCK_PORT="${MOCK_PORT:-18832}"
KEEP="${KEEP:-0}"
QA_ROOT="${QA_ROOT:-$(mktemp -d /tmp/aleph-qa-hpre-XXXXXX)}"
SERVER_PID=""
MOCK_PID=""

cleanup() {
    [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
    [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null || true
    [ -n "$SERVER_PID" ] && wait "$SERVER_PID" 2>/dev/null || true
    [ -n "$MOCK_PID" ] && wait "$MOCK_PID" 2>/dev/null || true
    if [ "$KEEP" != 1 ]; then rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT INT TERM

mkdir -p "$QA_ROOT"
HEAD_BEFORE="$(git -C "$REPO" rev-parse HEAD)"
BUILD_STARTED="$(date +%s)"
printf 'HEAD_BEFORE: %s\n' "$HEAD_BEFORE"
printf 'BUILD_COMMAND: %s build --bin aleph-server\n' "$GUARD"
# This is intentionally before qa_redirect_home: the guarded build uses the
# operator's real toolchain/cache and cannot silently use a stale target binary.
"$PYTHON" "$GUARD" build --bin aleph-server >"$QA_ROOT/build.log" 2>&1
BUILD_RC=$?
cat "$QA_ROOT/build.log"
[ "$BUILD_RC" -eq 0 ] || { echo "BUILD_RESULT: FAIL exit=$BUILD_RC"; exit "$BUILD_RC"; }
HEAD_AFTER="$(git -C "$REPO" rev-parse HEAD)"
[ "$HEAD_AFTER" = "$HEAD_BEFORE" ] || { echo "STALE_GUARD: HEAD changed during build"; exit 1; }
# `cargo build --bin aleph-server` uses this worktree's normal debug target.
# Do not select an arbitrary executable from a shared/alternate target tree.
BIN="$REPO/target/debug/aleph-server"
[ -x "$BIN" ] || { echo "STALE_GUARD: no built aleph-server at $BIN"; exit 1; }
BIN_MTIME="$(stat -f %m "$BIN")"
if [ "$BIN_MTIME" -lt "$BUILD_STARTED" ]; then
    # Cargo is allowed to report a fully fresh artifact without touching its
    # mtime.  Accept that only when the guarded build explicitly said so;
    # otherwise a pre-existing binary is not evidence for this tree.
    grep -Eq 'Fresh (aleph-server|aleph-server v)|Finished .*target' "$QA_ROOT/build.log" || {
        echo "STALE_GUARD: binary predates this build without a Cargo Fresh record"
        exit 1
    }
    printf 'BINARY_MTIME_GUARD: Fresh artifact (mtime=%s build_started=%s)\n' "$BIN_MTIME" "$BUILD_STARTED"
else
    printf 'BINARY_MTIME_GUARD: rebuilt artifact (mtime=%s build_started=%s)\n' "$BIN_MTIME" "$BUILD_STARTED"
fi
BIN_SHA="$(shasum -a 256 "$BIN" | awk '{print $1}')"
printf 'BINARY: %s\nBINARY_SHA256: %s\n' "$BIN" "$BIN_SHA"

# Scratch HOME is redirected only after the exact-tree build and stale checks.
. "$REPO/qa/lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
export REAL_HOME
CONFIG="$ALEPH_HOME/config.toml"
SERVER_LOG="$QA_ROOT/server.log"
REQUEST_LOG="$QA_ROOT/requests.jsonl"

# First boot writes the default config under the isolated HOME.
"$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/generate.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 60); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null || true
wait "$GEN_PID" 2>/dev/null || true
[ -f "$CONFIG" ] || { cat "$QA_ROOT/generate.log"; echo "SETUP_RESULT: FAIL no config"; exit 1; }
"$PYTHON" "$PATCH" "$CONFIG" --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1
# MCP mutation RPC is the existing public registry surface; leave it available
# to the real gateway handler even though no supplemental admin surface exists.
"$PYTHON" - "$CONFIG" <<'PY'
from pathlib import Path
p = Path(__import__('sys').argv[1])
s = p.read_text()
s = s.replace('mcp_enabled = false', 'mcp_enabled = true')
p.write_text(s)
PY

"$PYTHON" "$BUSY/mock_anthropic.py" "$MOCK_PORT" /etc/hostname single-shot "" "$REQUEST_LOG" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 40); do
    curl -sf -o /dev/null "http://127.0.0.1:$MOCK_PORT/v1/models" 2>/dev/null && break
    sleep 0.5
done
curl -sf -o /dev/null "http://127.0.0.1:$MOCK_PORT/v1/models" 2>/dev/null || { cat "$QA_ROOT/mock.log"; echo "SETUP_RESULT: SKIP provider unavailable"; exit 2; }

(cd "$QA_ROOT" && exec "$BIN" start) >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 90); do
    curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && break
    kill -0 "$SERVER_PID" 2>/dev/null || { cat "$SERVER_LOG"; exit 1; }
    sleep 1
done
curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null || { cat "$SERVER_LOG"; exit 1; }
printf 'SERVER_MODE: '; grep 'Mode:' "$SERVER_LOG" | tail -1 || true
case "$(grep 'Mode:' "$SERVER_LOG" | tail -1)" in
  *"Real AgentLoop"*) ;;
  *) echo "REAL_MODE_GUARD: FAIL"; cat "$SERVER_LOG"; exit 1 ;;
esac

WS="ws://127.0.0.1:$GATEWAY_PORT/ws"
RUN_ID="$(printf '%s' "$HEAD_BEFORE:$BIN_SHA:$$" | shasum -a 256 | awk '{print substr($1,1,16)}')"
RC=0
run_scenario() {
    local scenario="$1"
    local rc=0
    echo "SCENARIO_COMMAND[$scenario]: $PYTHON $HERE/drive.py --scenario $scenario"
    "$PYTHON" "$HERE/drive.py" --ws "$WS" --provider-log "$REQUEST_LOG" \
        --mcp-script "$MCP" --python "$PYTHON" --scenario "$scenario" \
        --prefix "qa_hpre_${RUN_ID}" --run-id "$RUN_ID" || rc=$?
    echo "SCENARIO_RESULT[$scenario]: exit=$rc"
    [ "$rc" -eq 0 ] || RC="$rc"
}
# Keep each required scenario named in output. ownership/close/overflow are
# expected to be UNVERIFIED when no legitimate external control exists.
run_scenario initial
run_scenario replacement
run_scenario ownership
run_scenario close
run_scenario overflow

printf 'QA_ROOT: %s\n' "$QA_ROOT"
printf 'OVERALL_RESULT: exit=%s\n' "$RC"
exit "$RC"
