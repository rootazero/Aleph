#!/usr/bin/env bash
# Real-machine QA: tools that pause for a human (F4).
#
#   ./qa/human_gate/run.sh ask         # ask_user, answered through clarification.resolve
#   ./qa/human_gate/run.sh text        # the same question over a text channel, one reply per question
#   ./qa/human_gate/run.sh unattended  # a cron run asks: refused at once, nothing left pending
#   ./qa/human_gate/run.sh stale       # a late reply to an already-answered request is reported stale
#
# One mock script for every stage (ask.tools.json): the model's first turn
# calls `ask_user` with two questions, the next ends the turn. The config gets
# a generic webhook channel (busy mode `queue`) so `text` can arrive the way a
# Telegram or Slack reply does.
#
# Same scratch-HOME discipline and shared helpers as qa/busy_input/run.sh.
set -uo pipefail

STAGE="${1:-ask}"
case "$STAGE" in ask|cancel|text|unattended|stale) ;; *) echo "unknown stage: $STAGE" >&2; exit 64 ;; esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SHARED="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-human-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18851}"
MOCK_PORT="${MOCK_PORT:-18852}"

# Build BEFORE HOME is redirected — see qa/README.md.
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
DB="$ALEPH_HOME/data/sessions.db"
REQUEST_LOG="$QA_ROOT/requests.jsonl"
SECRET="qa-webhook-secret"
WEBHOOK_PATH="/webhook/generic"
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
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" \
  --channel-busy-mode queue --channel-secret "$SECRET" --channel-path "$WEBHOOK_PATH" || exit 1
# The cron RPCs are registered only when the cron service is on; `unattended`
# drives a job through them. `cron.run` only queues the job for the service's
# next tick, so the tick is 1s here: the 60s budget in `unattended` must time
# the ask, not the scheduler. `~` in `db_path` resolves under the scratch HOME.
python3 - "$CONFIG" <<'EOF' || exit 1
import re, sys
path = sys.argv[1]
src = open(path).read()
new, n = re.subn(r"(\[cron\][^\[]*?)enabled = false", r"\1enabled = true", src, count=1)
if n != 1:
    sys.exit(f"patch: no `[cron] enabled = false` in {path}")
new, n = re.subn(r"(\[cron\][^\[]*?)check_interval_secs = \d+", r"\1check_interval_secs = 1", new, count=1)
if n != 1:
    sys.exit(f"patch: no `[cron] check_interval_secs` in {path}")
open(path, "w").write(new)
EOF

say "start mock provider (scripted: $STAGE)"
python3 "$SHARED/mock_anthropic.py" "$MOCK_PORT" /etc/hostname scripted \
  "$HERE/ask.tools.json" "$REQUEST_LOG" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
sleep 1

say "start server"
start_server || exit 1

RC=0
say "drive: $STAGE"
python3 "$HERE/drive.py" "$STAGE" "$WS_URL" "$DB" "$REQUEST_LOG" \
  "http://127.0.0.1:$GATEWAY_PORT" "$WEBHOOK_PATH" "$SECRET" || RC=$?

say "mock provider log"
tail -30 "$QA_ROOT/mock.log"

say "verdict: rc=$RC"
exit "$RC"
