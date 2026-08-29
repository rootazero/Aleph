#!/usr/bin/env bash
# Orchestrate one busy-input real-machine QA scenario end to end.
#
#   ./qa/busy_input/run.sh burst-drain     # §4.8 Round-9, RPC face
#   ./qa/busy_input/run.sh interrupt       # §4.8 Round-8 ①, channel inbound
#   ./qa/busy_input/run.sh queue           # channel inbound, nothing cancelled
#
# Everything lands in a scratch HOME/ALEPH_HOME under $QA_ROOT (default: a
# mktemp dir), so this never touches the developer's ~/.aleph — which matters
# more than it sounds: a second process on the same vault is a documented way
# to lose vault data (see PROCESS_MANAGEMENT.md).
set -uo pipefail

SCENARIO="${1:-burst-drain}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18793}"
MOCK_PORT="${MOCK_PORT:-18994}"
SECRET="qa-webhook-secret"
WEBHOOK_PATH="/webhook/generic"

# The build must run BEFORE HOME is redirected: cargo's registry, git cache and
# rustup toolchain all live under the real HOME, and a build launched with the
# scratch one silently degrades into a full network fetch that then times out.
# (It also grows a ~/.rustup inside the scratch dir, which is how this trap
# announces itself.)
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
# Redirects HOME/ALEPH_HOME into the scratch root AND pins RUSTUP_HOME/
# CARGO_HOME at the real ones — the redirect and the pin are inseparable
# on purpose; see that file for the 1.3 GB-per-run leak it closes.
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
DB="$ALEPH_HOME/data/sessions.db"

# The 32 MB floor in main.rs::worker_stack_size is not enough for a debug-built
# agent run with tools: it aborts with "tokio-rt-worker has overflowed its
# stack". Release builds do not need this.
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

BIN="$REPO/target/debug/aleph-server"
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

case "$SCENARIO" in
  burst-drain) PLAN=burst-drain; CHANNEL_MODE="" ;;
  interrupt)   PLAN=channel-burst; CHANNEL_MODE=interrupt ;;
  queue)       PLAN=channel-burst; CHANNEL_MODE=queue ;;
  steer)       PLAN=channel-burst; CHANNEL_MODE=steer ;;
  *) echo "unknown scenario: $SCENARIO" >&2; exit 64 ;;
esac

say "build ($SCENARIO)"
# A stale build-script cache can bake a deleted worktree path into the link
# line; `touch build.rs` is the documented cure and costs one rebuild.
# SKIP_BUILD=1 reuses whatever is already at $BIN.
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! qa_build --bin aleph-server; then
    echo "build failed" >&2; exit 1
  fi
fi
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

say "generate a baseline config"
# First boot writes the default config, then exits; we patch that rather than
# hand-writing one, so the fixture keeps working as the config schema moves.
timeout 25 "$BIN" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
PATCH_ARGS=(--gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT")
[ -n "$CHANNEL_MODE" ] && PATCH_ARGS+=(--channel-busy-mode "$CHANNEL_MODE" --channel-secret "$SECRET" --channel-path "$WEBHOOK_PATH")
python3 "$HERE/patch_config.py" "$CONFIG" "${PATCH_ARGS[@]}" || exit 1

say "start mock provider (plan $PLAN)"
python3 "$HERE/mock_anthropic.py" "$MOCK_PORT" /etc/hostname "$PLAN" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
sleep 1

say "start server"
# stdout is not a TTY here, so server tracing goes to $ALEPH_HOME/logs/ — the
# redirect below catches only the startup banner. Look in logs/ for anything real.
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
if [ "$SCENARIO" = "burst-drain" ]; then
  python3 "$HERE/drive_burst_drain.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$DB" || RC=$?
else
  python3 "$HERE/drive_channel_busy.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" "http://127.0.0.1:$GATEWAY_PORT" \
    "$DB" "$SECRET" "$WEBHOOK_PATH" "$CHANNEL_MODE" || RC=$?
fi

say "mock provider log"
tail -30 "$QA_ROOT/mock.log"

say "server log tail"
LOGDIR="$ALEPH_HOME/logs"
if [ -d "$LOGDIR" ]; then
  tail -40 "$LOGDIR"/aleph-server.log* 2>/dev/null | tail -40
else
  tail -20 "$QA_ROOT/server.log"
fi

say "verdict: rc=$RC"
exit "$RC"
