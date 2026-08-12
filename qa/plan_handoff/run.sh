#!/usr/bin/env bash
# Orchestrate one plan→build handoff real-machine QA scenario end to end.
#
#   ./qa/plan_handoff/run.sh handoff   # refuse -> card -> approve -> unlock, one run
#   ./qa/plan_handoff/run.sh deny      # declined card leaves the floor engaged
#   ./qa/plan_handoff/run.sh floor     # explicit `allow` + `full` tier still lose
#
# Same scratch-HOME discipline as qa/busy_input/run.sh, and it reuses that
# scenario's `patch_config.py` and `lib.py` rather than growing a second copy —
# the config-shaping and the session-log clock are not plan-specific.
set -uo pipefail

SCENARIO="${1:-handoff}"
case "$SCENARIO" in handoff|deny|floor) ;; *) echo "unknown scenario: $SCENARIO" >&2; exit 64 ;; esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SHARED="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-plan-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18795}"
MOCK_PORT="${MOCK_PORT:-18996}"

# Build BEFORE HOME is redirected — cargo's registry, git cache and rustup
# toolchain all live under the real HOME. See qa/README.md.
REAL_HOME="$HOME"

export HOME="$QA_ROOT/home"
export ALEPH_HOME="$QA_ROOT/home/.aleph"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
DB="$ALEPH_HOME/data/sessions.db"
OBS="$QA_ROOT/observations.jsonl"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

# NOT `$REPO/target`: this repo resolves a shared target directory, so a git
# worktree builds into the MAIN checkout's target tree and `$REPO/target` does
# not exist at all. Ask cargo instead of assuming — with the real HOME, since
# `cargo metadata` reads the registry.
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --no-deps --format-version 1 2>/dev/null \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])' 2>/dev/null)"
BIN="${TARGET_DIR:-$REPO/target}/debug/aleph-server"
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

say "build ($SCENARIO)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! (cd "$REPO" && HOME="$REAL_HOME" cargo build --bin aleph-server 2>&1 | tail -5); then
    echo "build failed" >&2; exit 1
  fi
fi
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

say "generate a baseline config"
timeout 25 "$BIN" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
python3 "$SHARED/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1

if [ "$SCENARIO" = "floor" ]; then
  # The claim under test: an explicit entry beats the TIER (by design) and
  # must not beat the FLOOR. Written the way an operator would write it —
  # INTO the generated config's existing overrides table, not appended as a
  # second one; see add_overrides.py.
  python3 "$HERE/add_overrides.py" "$CONFIG" bash=allow file_write=allow || exit 1
fi

say "start mock provider (plan $SCENARIO)"
python3 "$HERE/mock_plan.py" "$MOCK_PORT" "$SCENARIO" "$OBS" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
sleep 1

say "start server"
"$BIN" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 90); do
  if curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null; then break; fi
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

say "drive: $SCENARIO"
RC=0
python3 "$HERE/drive_plan_handoff.py" \
  "ws://127.0.0.1:$GATEWAY_PORT/ws" "$DB" "$OBS" "$SCENARIO" || RC=$?

say "mock provider log"
tail -40 "$QA_ROOT/mock.log"

say "server log tail"
LOGDIR="$ALEPH_HOME/logs"
if [ -d "$LOGDIR" ]; then
  grep -iE "plan|approval|refus" "$LOGDIR"/aleph-server.log* 2>/dev/null | tail -25
else
  tail -20 "$QA_ROOT/server.log"
fi

say "verdict: rc=$RC"
exit "$RC"
