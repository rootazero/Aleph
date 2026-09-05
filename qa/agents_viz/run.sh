#!/usr/bin/env bash
# Real-machine QA for the tasks + agents visualization round (FEATURE_LOCATOR
# §4.11 / §5.13 / §3.13): the wires a TUI tasks/agents panel and the Panel's
# /dashboard/subagents view are built on, driven against a REAL gateway with a
# deterministic mock provider.
#
#   ./qa/agents_viz/run.sh            # = claims
#   ./qa/agents_viz/run.sh claims     # the RPC-face claims below, all asserted by EFFECT
#   ./qa/agents_viz/run.sh panel      # boot + hold: point a browser at
#                                     # /dashboard/subagents, then trigger a
#                                     # delegation with the printed command
#   KEEP=1 …                          # keep the scratch dir for post-mortem
#   SKIP_BUILD=1 …                    # reuse the server binary already built
#
# The round fixed two SEVERED wires (判据 §7) — both ends existed, both had
# tests, and nothing connected them:
#
#   1. `shared/client::classify_frame` dropped every `{"method":"event"}` topic
#      frame with a `debug!`, so `run.subagent_tree` was structurally
#      undeliverable to the TUI. The TUI's connection is UNFILTERED (it never
#      calls `events.subscribe`), and `should_receive` answers "everything"
#      for a connection with no filter.
#   2. The Panel's subagents view never called `subscribe_topic`, and a
#      connection that HAS a filter (the Panel seeds one at connect) receives
#      only what it subscribed to — the view was snapshot-only.
#
# What `claims` proves, each one an effect a unit test cannot assert:
#
#   D1  an unfiltered connection receives `run.subagent_tree` in the exact
#       double-nested envelope the client now classifies (`method:"event"`,
#       `params.topic`) — the envelope is a wire key, not a detail (§10)
#   D2  the spawned node carries `child_session` and names the parent session
#       as `root_session` (the field the agent-run view opens `chat.history` on)
#   D3  the settled frame arrives with `lifecycle:"completed"` and a numeric
#       `total_tokens`
#   D4  a filtered connection that subscribed to the topic receives the same
#       frames — the Panel's path
#   D5  a filtered connection that did NOT subscribe receives none of them —
#       the negative arm that proves the subscription is the carrier, so D4
#       cannot be green by accident
#   D6  `chat.history` on the child's `child_session` returns the child's turn
#       — the "no new RPC" decision the round made, exercised.
#       ⚠️ RED on first measurement (2026-09-02) and kept red on purpose: the
#       child's events are all in `session_events`, but no `sessions` row
#       exists for the key, so `chat.history` answers `session not found`.
#       The TUI/Panel detail views therefore show "Transcript unavailable"
#       for every background child. FEATURE_LOCATOR 附录 D.4.37 has the
#       evidence and the candidate fix; until it lands this claim is the
#       falsifying arm, not a fixture bug.
#   P1  a mutating `scratchpad` call's snapshot rides the live trace/tool_end
#       frames of the run
#   P2  `RunSummary.plan` at `stream.run_complete` carries the same items
#   P3  `chat.history.plan` (cold) carries the same items
#
# What it deliberately does NOT prove: anything about the terminal. No pty is
# allocated and `aleph-tui` is never launched — the same call `qa/btw_tui`
# made, for the same reason (there is no pty rig in this repo, and the panels'
# rendering + key tables are covered in-process by `widgets::{tasks_panel,
# agents_panel,agents_overlay}::tests`). The Panel's rendering IS checked, by a
# human or an agent with a browser, through the `panel` scenario.
#
# Reuses qa/teamchat_rooms' mock provider (its `QA-DELEGATE` arm plus a
# `QA-PLAN` arm added for this fixture) and config patcher rather than growing
# a second copy of either; qa/lib for the scratch-HOME discipline.
set -uo pipefail

SCENARIO="${1:-claims}"
case "$SCENARIO" in
  claims|panel) ;;
  *) echo "unknown scenario: $SCENARIO (want: claims | panel)" >&2; exit 2 ;;
esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SHARED="$HERE/../teamchat_rooms"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18821}"
MOCK_PORT="${MOCK_PORT:-18921}"

command -v node >/dev/null 2>&1 || { echo "node is required for this fixture" >&2; exit 1; }

. "$HERE/../lib/build.sh"

# Build BEFORE the HOME redirect — see qa/teamchat_rooms/run.sh for why the
# per-command `HOME="$REAL_HOME"` guard is not enough on Windows.
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  echo "=== build ==="
  qa_build --bin aleph-server || { echo "server build failed" >&2; exit 1; }
fi
TARGET_DIR="$(cd "$REPO" && cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | node -e 'let s="";process.stdin.on("data",c=>s+=c).on("end",()=>console.log(JSON.parse(s).target_directory))')"
SERVER="$TARGET_DIR/debug/aleph-server"
[ -x "$SERVER" ] || SERVER="$SERVER.exe"
[ -x "$SERVER" ] || { echo "no server binary under $TARGET_DIR/debug" >&2; exit 1; }

# Mixed-form root on Windows: the native server reads `C:/…`, not `/c/…`.
if [ -z "${QA_ROOT:-}" ]; then
  QA_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-agentsviz-XXXXXX")"
  command -v cygpath >/dev/null 2>&1 && QA_ROOT="$(cygpath -m "$QA_ROOT")"
fi
mkdir -p "$QA_ROOT"

. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
export REAL_HOME
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
WORKSPACE="$QA_ROOT/workspace"
REQUEST_LOG="$QA_ROOT/requests.jsonl"
# The shared mock takes a delete target for its QA-CARD arm; unused here.
DELETE_TARGET="$WORKSPACE/unused.txt"
# A debug server overflows its worker stack on the first turn without this.
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

mkdir -p "$WORKSPACE"
: > "$REQUEST_LOG"

SERVER_PID=""
MOCK_PID=""
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

echo "=== generate a baseline config ==="
# `--port` on the GENERATION boot. The config does not exist yet, so without
# it this boot binds the built-in default port — and if anything already holds
# that port (another fixture, a dev server, the operator's own daemon) the
# process exits before writing a config at all. The symptom is
# `no config generated at …`, which reads like a permissions or path problem;
# the cause is one line further up the log. Binding the port this run already
# owns makes the generation boot as isolated as the real one.
timeout 40 "$SERVER" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 60); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -30 "$QA_ROOT/gen.log"; exit 1; }
node "$SHARED/patch_config.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" || exit 1

echo "=== start the mock provider ==="
node "$SHARED/mock_llm.mjs" "$MOCK_PORT" "$REQUEST_LOG" "$DELETE_TARGET" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 40); do
  curl -s -o /dev/null "http://127.0.0.1:$MOCK_PORT/v1/models" && break
  sleep 0.25
done

echo "=== start the server ==="
"$SERVER" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
UP=0
for _ in $(seq 1 120); do
  if curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null; then UP=1; break; fi
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
# "I could not ask" must never render as "the answer is no".
[ "$UP" = "1" ] || { echo "HARNESS_GATEWAY_NEVER_CAME_UP"; tail -40 "$QA_ROOT/server.log"; exit 1; }
echo "gateway up on $GATEWAY_PORT"

RC=0
case "$SCENARIO" in
  claims)
    echo "=== drive ==="
    node "$HERE/drive_agents_viz.mjs" "$GATEWAY_PORT" "$REQUEST_LOG" claims
    RC=$?
    if [ "$RC" != "0" ]; then
      echo; echo "--- server log tail ---"; tail -60 "$QA_ROOT/server.log"
      echo "--- mock log tail ---"; tail -40 "$QA_ROOT/mock.log"
    fi
    ;;
  panel)
    # Boot + hold. Every `claims` assertion is an RPC round-trip, so it says
    # nothing about whether the Panel's subagents view RENDERS the frames it
    # now subscribes to. That needs a browser attached BEFORE the delegation
    # runs — the tree is a live projection, and a node that settled before
    # the view mounted is only ever a snapshot row.
    SESSION_KEY="$(node "$HERE/drive_agents_viz.mjs" "$GATEWAY_PORT" "$REQUEST_LOG" session | tail -1)"
    cat <<EOF

=== panel scenario: holding ===
  Panel:      http://127.0.0.1:$GATEWAY_PORT/dashboard/subagents
  session:    $SESSION_KEY
  delegate:   node "$HERE/drive_agents_viz.mjs" $GATEWAY_PORT "$REQUEST_LOG" delegate "$SESSION_KEY"
  plan:       node "$HERE/drive_agents_viz.mjs" $GATEWAY_PORT "$REQUEST_LOG" plan "$SESSION_KEY"
  server log: $QA_ROOT/server.log
  stop:       touch "$QA_ROOT/stop"   (or Ctrl-C)
EOF
    while [ ! -f "$QA_ROOT/stop" ]; do sleep 1; done
    ;;
esac

exit "$RC"
