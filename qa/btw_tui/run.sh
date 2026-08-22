#!/usr/bin/env bash
# Real-machine QA for the wire the TUI's `/btw` overlay is built on.
#
#   ./qa/btw_tui/run.sh [frames|promote]     (default: frames)
#
# Two scenarios, because they need opposite things from the mock provider and
# a single run cannot have both:
#
#   frames  — the overlay's four claims about the wire. Needs a MAIN run that
#             stays alive across the whole side question (`channel-burst`),
#             since a side question exists to be asked WHILE one runs.
#   promote — the one crossing back. Needs a side question that COMPLETES
#             (`quick`), since there is nothing to promote until one has.
#
# What this proves, and what it deliberately does not:
#
#   PROVES — against a real gateway and a real engine — the facts neither
#   scenario can verify in process, because an in-process test supplies the
#   very frames the code expects and runs on an engine with no orchestrator
#   (see each driver's docstring for its own list).
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

SCENARIO="${1:-frames}"
case "$SCENARIO" in
  frames)  MOCK_PLAN="channel-burst" ;;
  promote) MOCK_PLAN="quick" ;;
  *) echo "unknown scenario: $SCENARIO (want: frames | promote)" >&2; exit 2 ;;
esac

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
# Armed HERE — immediately after `$QA_ROOT` exists and before the HOME
# redirect — rather than further down where the servers start. Nothing between
# `mktemp -d` and this line can leak the scratch root, which is the general
# shape this repo has already paid 7,623 orphaned trees and 4.0 GB for (see
# `utils::scratch`). It is a narrowing, not a bug fix: without `set -e` the
# statements in that window do not abort the script anyway.
#
# `cleanup` is on EXIT and EXIT only — the same single line as the other nine
# `qa/*/run.sh` fixtures, and it is enough on its own: a Ctrl-C runs the EXIT
# trap and exits 130 on both bashes this fixture can run under here (measured
# 2026-08-21 on 3.2.57 and 5.3.15, the system bash and the brew one).
#
# The one extra line is `exit`, not `cleanup`. A handler that *returns* is what
# breaks this: bash runs the handler at the interrupt and then RESUMES at the
# next statement, so a Ctrl-C during the 45 s observation window cleaned up and
# then ran the three diagnostic dumps against the root it had just deleted —
# destroying exactly the logs the operator interrupted the run to read — before
# cleaning up a second time at EXIT. `exit 130` makes the interrupt terminate
# the script, which fires the EXIT trap once, which cleans up once.
trap cleanup EXIT
trap 'exit 130' INT

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

say "start mock provider ($MOCK_PLAN)"
# `channel-burst` has a long flat tail of slow tool turns, which is what keeps
# the MAIN run alive across the side question — the whole point of `/btw`.
# `quick` does the opposite for the promote scenario: the turn counter is
# global, so a plan that ends lets the side question finish, and only a
# finished one is promotable.
python3 "$SHARED/mock_anthropic.py" "$MOCK_PORT" /etc/hostname "$MOCK_PLAN" \
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

say "drive ($SCENARIO)"
RC=0
if [ "$SCENARIO" = "promote" ]; then
  python3 "$HERE/drive_btw_promote.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" \
    --db "$ALEPH_HOME/data/sessions.db" || RC=$?
else
  python3 "$HERE/drive_btw_frames.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" || RC=$?
fi

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
