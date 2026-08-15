#!/usr/bin/env bash
# Orchestrate one background-bash announce real-machine QA scenario.
#
#   ./qa/announce/run.sh outlive     # the job outlives its run -> a fresh run is driven
#   ./qa/announce/run.sh collected   # the model collected it -> no turn is spent
#   ./qa/announce/run.sh midrun      # the run is still alive -> absorbed, ONE run
#
# Everything lands in a scratch HOME/ALEPH_HOME under $QA_ROOT, so this never
# touches the developer's ~/.aleph (two processes on one vault is a documented
# way to lose vault data — PROCESS_MANAGEMENT.md).
set -uo pipefail

SCENARIO="${1:-outlive}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
PLANH="$HERE/../plan_handoff"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-announce-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18795}"
MOCK_PORT="${MOCK_PORT:-18996}"
# How long the backgrounded command sleeps. It has to outlast the run that
# spawns it in `outlive`/`collected`, and be comfortably shorter than the
# `midrun` plan's think-time. Nothing here waits on a wall-clock guess for a
# CLAIM — the driver waits on the session log and on the mock's observations —
# but this one number does have to be a duration, because it is the thing being
# outlived.
SLEEP_SECS="${SLEEP_SECS:-12}"

case "$SCENARIO" in
  outlive|collected|midrun) ;;
  *) echo "unknown scenario: $SCENARIO (outlive|collected|midrun)" >&2; exit 64 ;;
esac

# Build BEFORE HOME is redirected: cargo's registry, git cache and rustup
# toolchain all live under the real HOME, and a build launched with the scratch
# one silently degrades into a full network fetch that then times out.
REAL_HOME="$HOME"

export HOME="$QA_ROOT/home"
export ALEPH_HOME="$QA_ROOT/home/.aleph"   # the .aleph dir ITSELF, not its parent
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
DB="$ALEPH_HOME/data/sessions.db"
OBS="$QA_ROOT/observations.jsonl"

# The 32 MB floor in main.rs::worker_stack_size is not enough for a debug-built
# agent run with tools.
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
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build ($SCENARIO)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! (cd "$REPO" && HOME="$REAL_HOME" cargo build --bin aleph-server 2>&1 | tail -5); then
    echo "build failed" >&2; exit 1
  fi
fi
# Ask cargo where its target dir really is: `.cargo/config.toml` pins a shared
# absolute one, so a hardcoded `$REPO/target` is wrong from any git worktree.
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

say "generate a baseline config"
timeout 25 "$BIN" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
# `--max-pending-steering 8`: busy_input pins it to 1 to make backpressure one
# message away, which is the opposite of what `midrun` needs — the notice must
# be ABSORBED, and a cap of 1 would have it refused for backpressure instead.
python3 "$BUSY/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" \
  --max-pending-steering 8 --wake-fallback-secs 600 || exit 1
# `bash` is not idempotent, so the default `auto` tier raises a confirmation
# card for it and the run would park on a human who is not there. An explicit
# `allow` entry outranks the tier (`effective_permission`) — the same knob an
# operator would use, not a test-only bypass.
python3 "$PLANH/add_overrides.py" "$CONFIG" bash=allow || exit 1

say "start mock provider (scenario $SCENARIO, sleep ${SLEEP_SECS}s)"
python3 "$HERE/mock_announce.py" "$MOCK_PORT" "$SCENARIO" "$OBS" "$SLEEP_SECS" \
  >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
sleep 1

say "start server"
# stdout is not a TTY here, so tracing goes to $ALEPH_HOME/logs/ — the redirect
# below catches only the startup banner. "No output" is not "nothing happened".
"$BIN" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 90); do
  curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && break
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

say "drive: $SCENARIO"
RC=0
python3 "$HERE/drive_announce.py" \
  "ws://127.0.0.1:$GATEWAY_PORT/ws" "$DB" "$OBS" "$SCENARIO" "$SLEEP_SECS" || RC=$?

say "mock provider log"
tail -30 "$QA_ROOT/mock.log"

say "announce-related server log lines"
LOGDIR="$ALEPH_HOME/logs"
if [ -d "$LOGDIR" ]; then
  grep -iE 'announce|background|process_completed|ProcessCompleted' "$LOGDIR"/aleph-server.log* 2>/dev/null \
    | tail -25 || echo "(no announce lines)"
else
  tail -20 "$QA_ROOT/server.log"
fi

say "verdict: rc=$RC"
exit "$RC"
