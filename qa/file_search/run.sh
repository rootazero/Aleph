#!/usr/bin/env bash
# Real-machine QA for the `grep` / `find` builtins (§3.4/§3.6).
#
#   ./qa/file_search/run.sh floor   # the deny floor binds, and no_ignore does not lift it
#   ./qa/file_search/run.sh page    # the window reports the whole, and pages are disjoint
#   ./qa/file_search/run.sh reach   # a real agent turn's grep output reaches the model
#   ./qa/file_search/run.sh steer   # a shell `grep -r` is steered; a bounded
#                                   # `grep` and a shell `rg` are not
#
#   KEEP=1 ./qa/file_search/run.sh floor    # keep the scratch dir for post-mortem
#
# ## Why any of this needs a booted server
#
# Every unit test beside these tools calls `GrepTool::run` directly and hands
# `denied_paths` in by hand. That is blind to two whole classes of failure:
#
#   * a tool registered on three faces and dispatched on none — the shape
#     `plugin_manage` shipped in, which sixteen thousand in-process tests could
#     not see because each one correctly answered "yes, it is registered";
#   * `[sandbox] deny_read_globs` never reaching `get_denied_paths()`. The unit
#     tests prove the predicate; only a config file on disk proves the wiring.
#
# The `floor` and `page` phases go through `tools.invoke`, which dispatches
# straight off the live `ToolRegistry` and needs no model. `reach` and `steer`
# spend a real agent turn, because their claim is about what came back INTO the
# conversation — which is a different object on a different path from the RPC
# reply, and the mock provider's request log is its only oracle.
set -uo pipefail

PHASE="${1:-floor}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
PLANH="$HERE/../plan_handoff"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-filesearch-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18801}"
MOCK_PORT="${MOCK_PORT:-18802}"
TREE="$QA_ROOT/tree"

case "$PHASE" in
  floor|page|reach|steer) ;;
  *) echo "unknown phase: $PHASE (floor|page|reach|steer)" >&2; exit 64 ;;
esac

# Build BEFORE HOME is redirected: cargo's registry, git cache and rustup
# toolchain all live under the real HOME, and a build launched with the scratch
# one silently degrades into a full network fetch.
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"

# A debug-built agent turn with tools overflows the 32 MB worker stack floor.
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

SERVER_PID=""
MOCK_PID=""
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

say "plant the trees"
EXPECT="$(python3 "$HERE/plant_tree.py" "$TREE")" || exit 1
NEEDLE="$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["needle"])' "$EXPECT")"
echo "$EXPECT"

say "build ($PHASE)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  qa_build --bin aleph-server || { echo "build failed" >&2; exit 1; }
fi
# Ask cargo where its target dir really is rather than assuming `$REPO/target`:
# a shared absolute target-dir makes the guess wrong from any git worktree.
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }
# The one trace a swallowed build failure leaves behind. See qa/lib/build.sh.
echo "binary: $BIN ($(date -r "$BIN" '+%Y-%m-%d %H:%M:%S'))"

say "generate a baseline config"
# `--port` on the GENERATION boot. The config does not exist yet, so without
# it this boot binds the built-in default port — and if anything already holds
# that port (another fixture, a dev server, the operator's own daemon) the
# process exits before writing a config at all. The symptom is
# `no config generated at …`, which reads like a permissions or path problem;
# the cause is one line further up the log. Binding the port this run already
# owns makes the generation boot as isolated as the real one.
timeout 25 "$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
python3 "$BUSY/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" --max-pending-steering 8 || exit 1
# The live-config half of the `floor` claim. `get_denied_paths()` snapshots
# this once per process, so it has to be on disk before the server boots —
# which is exactly why a unit test cannot make this claim.
python3 "$HERE/patch_sandbox.py" "$CONFIG" '**/*.pem' || exit 1
if [ "$PHASE" = "steer" ]; then
  # `bash` is not idempotent, so the default `auto` tier raises a confirmation
  # card and the run would park on a human who is not there. An explicit
  # `allow` outranks the tier — the operator's own knob, not a test bypass.
  # Deliberately NOT granted to `grep`: if the read-only tools needed an
  # override to run in a turn, that would itself be worth discovering.
  python3 "$PLANH/add_overrides.py" "$CONFIG" bash=allow || exit 1
fi

RC=0
case "$PHASE" in
  floor|page)
    say "start server"
    "$BIN" start >"$QA_ROOT/server.log" 2>&1 &
    SERVER_PID=$!
    ;;
  reach|steer)
    say "write the turn's tool plan"
    if [ "$PHASE" = "reach" ]; then
      python3 - "$QA_ROOT/spec.json" "$NEEDLE" "$TREE/probe" <<'PY'
import json, sys
out, needle, probe = sys.argv[1], sys.argv[2], sys.argv[3]
json.dump({"name": "grep", "input": {"pattern": needle, "path": probe}}, open(out, "w"))
PY
    else
      # One steered arm and TWO controls. The controls are the half that
      # matters: a steer appended to every shell command would satisfy the
      # positive arm and be worthless. They are two because they fail
      # differently.
      python3 - "$QA_ROOT/spec.json" "$NEEDLE" "$TREE/probe" <<'PY'
import json, sys
out, needle, probe = sys.argv[1], sys.argv[2], sys.argv[3]
# The trailing `echo` is what lets the driver attribute a tool_result to its
# arm without counting turns (a run opens with a strategy-planner call, so
# turn numbers do not line up with the plan). `search_steer` classifies the
# FIRST pipeline segment only, so a second segment cannot change the verdict.
#
# QA_ARM_BOUNDED is the load-bearing control. It holds the PROGRAM constant
# and varies only the property the classifier keys on — recursion — so a
# green here means the classifier discriminated, not that some earlier layer
# declined to run anything. `grep` is on every machine this can run on, so
# the arm also produces real matches, and the driver checks for them before
# it checks for the absence of a steer: `STEER not in output` is satisfied
# just as well by an output that never reached the classifier at all.
#
# QA_ARM_RG covers the carve-out written into `search_steer`'s module doc —
# `bash`'s own description recommends `rg`, so steering it would make the two
# surfaces contradict each other. It CANNOT be load-bearing, because `rg` is
# not installed on every machine and is not installed on this one (the shell
# answers `rg: command not found`). The predicate under test is syntactic, so
# the arm is still meaningful where `rg` exists; where it does not, the driver
# reports SKIP rather than a pass it did not earn.
json.dump(
    [
        {"name": "bash",
         "input": {"cmd": f"grep -rn {needle} {probe} ; echo QA_ARM_STEERED"}},
        {"name": "bash",
         "input": {"cmd": f"grep -n {needle} {probe}/src/alpha.rs ; echo QA_ARM_BOUNDED"}},
        {"name": "bash",
         "input": {"cmd": f"rg {needle} {probe} ; echo QA_ARM_RG"}},
    ],
    open(out, "w"),
)
PY
    fi
    say "start mock provider"
    # `tool-chain`, not `quick`: a run opens with a strategy-planner call that
    # carries no tool surface and still advances the mock's counter, so a
    # two-entry plan is spent before the agent turn that matters.
    python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" /etc/hostname tool-chain \
      "$QA_ROOT/spec.json" "$QA_ROOT/requests.jsonl" >"$QA_ROOT/mock.log" 2>&1 &
    MOCK_PID=$!
    sleep 1
    say "start server"
    "$BIN" start >"$QA_ROOT/server.log" 2>&1 &
    SERVER_PID=$!
    ;;
esac

for _ in $(seq 1 90); do
  curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && break
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

say "drive: $PHASE"
case "$PHASE" in
  floor|page)
    python3 "$HERE/drive_invoke.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$PHASE" "$TREE" "$EXPECT" || RC=$?
    ;;
  reach|steer)
    python3 "$HERE/drive_turn.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$PHASE" \
      "$QA_ROOT/requests.jsonl" "$NEEDLE" || RC=$?
    say "mock provider log"
    tail -20 "$QA_ROOT/mock.log"
    ;;
esac

say "server log tail"
LOGDIR="$ALEPH_HOME/logs"
if [ -d "$LOGDIR" ]; then
  tail -30 "$LOGDIR"/aleph-server.log* 2>/dev/null | tail -30
else
  tail -20 "$QA_ROOT/server.log"
fi

say "verdict: rc=$RC"
exit "$RC"
