#!/usr/bin/env bash
# Real-machine QA for the wire the TUI's `/btw` overlay is built on.
#
#   ./qa/btw_tui/run.sh
#
# What this proves, and what it deliberately does not:
#
#   PROVES — against a real gateway and a real engine — the four facts about
#   the frames that `interfaces/tui/src/tui/btw_overlay.rs` cannot verify in
#   process, because an in-process test supplies the very frames the code
#   expects (see `drive_btw_frames.py`'s docstring for the list).
#
#   DOES NOT PROVE — anything about the terminal. No pty is allocated and
#   `aleph-tui` is never launched: the overlay's rendering and its key table
#   are covered by `tui::btw_key_tests` and `widgets::btw_panel::tests`
#   in-process, and a screen-scraping pty rig (there is no existing one in this
#   repo to extend) was judged disproportionate to what it would add over
#   those. The gap it leaves is stated in the task report.
#
# Everything lands in a scratch HOME/ALEPH_HOME under $QA_ROOT, and it reuses
# qa/busy_input's mock provider, config patcher and WS helpers rather than
# growing a second copy of any of them.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SHARED="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-btw-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18797}"
MOCK_PORT="${MOCK_PORT:-18998}"

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
# Armed HERE — immediately after `$QA_ROOT` exists and before anything that
# can fail — rather than further down where the servers start.
#
# The window it closes is small and the leak it prevents is not: every failure
# between `mktemp -d` and the trap left a scratch HOME behind, and the
# reproducing case is the ordinary one (`qa_redirect_home` or `mkdir` failing
# on a full or read-only $TMPDIR). This repo has already paid for the general
# version of this — 7,623 orphaned trees and 4.0 GB from guards that dropped
# before the thing they guarded (see `utils::scratch`).
#
# INT/TERM/HUP as well as EXIT: a Ctrl-C during the 45 s observation window is
# the single likeliest way this script ends, and whether a bare EXIT trap runs
# on a fatal signal is shell- and version-dependent. `- ` re-raises nothing; the
# explicit list is what makes it not depend on that.
trap cleanup EXIT INT TERM HUP

# The build must run BEFORE HOME is redirected — cargo's registry, git cache
# and rustup toolchain all live under the real HOME. See qa/README.md.
. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"

# A debug-built agent run with tools overflows main.rs's 32 MB worker stack.
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

BIN="$REPO/target/debug/aleph-server"

say "build"
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

say "start mock provider"
# `channel-burst` has a long flat tail of slow tool turns, which is what keeps
# the MAIN run alive across the side question — the whole point of `/btw`.
python3 "$SHARED/mock_anthropic.py" "$MOCK_PORT" /etc/hostname channel-burst \
  >"$QA_ROOT/mock.log" 2>&1 &
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

say "drive"
RC=0
python3 "$HERE/drive_btw_frames.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" || RC=$?

say "mock provider log"
tail -20 "$QA_ROOT/mock.log"

say "server log tail"
LOGDIR="$ALEPH_HOME/logs"
if [ -d "$LOGDIR" ]; then
  tail -30 "$LOGDIR"/aleph-server.log* 2>/dev/null | tail -30
else
  tail -20 "$QA_ROOT/server.log"
fi

say "verdict: rc=$RC"
exit "$RC"
