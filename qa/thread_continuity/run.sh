#!/usr/bin/env bash
# Real-machine QA: one conversation thread across restarts and clients.
#
#   ./qa/thread_continuity/run.sh reopen       # F1: knobs, model pin, usage survive a restart
#   ./qa/thread_continuity/run.sh checklist    # F2: the checklist gates the finish, survives a restart
#   ./qa/thread_continuity/run.sh two_clients  # F8: two clients send to one thread at once
#   ./qa/thread_continuity/run.sh fork         # F7: fork siblings share a prefix; isolated sees no parent
#
# A stage that needs a restart runs the driver twice — phase `before`, then
# this script stops the server with SIGTERM and starts it again on the SAME
# ALEPH_HOME, then phase `after`. State between the phases goes through
# $QA_ROOT/state.json. The mock provider keeps running across the restart, so
# its scripted turn counter and request log cover the whole stage.
#
# Same scratch-HOME discipline and shared helpers as qa/busy_input/run.sh.
set -uo pipefail

STAGE="${1:-reopen}"
case "$STAGE" in reopen|checklist|two_clients|fork) ;; *) echo "unknown stage: $STAGE" >&2; exit 64 ;; esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SHARED="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-thread-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18797}"
MOCK_PORT="${MOCK_PORT:-18998}"

# Build BEFORE HOME is redirected — see qa/README.md.
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
DB="$ALEPH_HOME/data/sessions.db"
REQUEST_LOG="$QA_ROOT/requests.jsonl"
STATE="$QA_ROOT/state.json"
WS_URL="ws://127.0.0.1:$GATEWAY_PORT/ws"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

MOCK_PID=""
SERVER_PID=""

say() { printf '\n=== %s ===\n' "$*"; }

cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then
    echo "artifacts kept in $QA_ROOT"
  else
    rm -rf "$QA_ROOT"
  fi
}
trap cleanup EXIT

start_server() {
  "$BIN" start >>"$QA_ROOT/server.log" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 90); do
    if curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null; then
      echo "gateway up on $GATEWAY_PORT (pid $SERVER_PID)"
      return 0
    fi
    kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; return 1; }
    sleep 1
  done
  echo "server never became healthy"; return 1
}

# A graceful stop, the way a user closes the app: SIGTERM, then wait for the
# process to really exit, so the next start is not racing the old one for the
# port or the instance lock.
restart_server() {
  say "restart server (SIGTERM, same ALEPH_HOME)"
  kill -TERM "$SERVER_PID" 2>/dev/null
  for _ in $(seq 1 60); do
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.5
  done
  if kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "server did not exit within 30s of SIGTERM"; return 1
  fi
  wait "$SERVER_PID" 2>/dev/null
  start_server
}

say "build ($STAGE)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  qa_build --bin aleph-server || { echo "build failed" >&2; exit 1; }
fi
TARGET_DIR="$(cd "$REPO" && HOME="${REAL_HOME:-$HOME}" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

say "generate a baseline config"
timeout 25 "$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
python3 "$SHARED/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1
# A second model on the mock provider, so pinning one is an observable change
# in every later request's `model` field rather than a no-op on the default.
python3 - "$CONFIG" <<'EOF' || exit 1
import sys
path = sys.argv[1]
src = open(path).read()
old = 'models = ["qa-mock-model"]'
if old not in src:
    sys.exit(f"patch: {old!r} not found in {path}")
open(path, "w").write(src.replace(old, 'models = ["qa-mock-model", "qa-mock-model-b"]', 1))
EOF

say "start mock provider (scripted: $STAGE)"
python3 "$SHARED/mock_anthropic.py" "$MOCK_PORT" /etc/hostname scripted \
  "$HERE/$STAGE.tools.json" "$REQUEST_LOG" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
sleep 1

say "start server"
start_server || exit 1

drive() {
  python3 "$HERE/drive.py" "$STAGE" "$1" "$WS_URL" "$DB" "$REQUEST_LOG" "$STATE"
}

RC=0
case "$STAGE" in
  reopen|checklist)
    say "drive: $STAGE before"
    drive before || RC=$?
    restart_server || exit 1
    say "drive: $STAGE after"
    drive after || RC=$((RC + $?))
    ;;
  *)
    say "drive: $STAGE"
    drive only || RC=$?
    ;;
esac

say "mock provider log"
tail -30 "$QA_ROOT/mock.log"

say "verdict: rc=$RC"
exit "$RC"
