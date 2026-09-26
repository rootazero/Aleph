#!/usr/bin/env bash
# Real-machine QA for the context-slim round (reasoning replay policy +
# tool-output ingress slimming + server-side context editing).
#
#   ./qa/context_slim/run.sh replay      # DeepSeek thinking+tools: every prior turn's reasoning_content is sent back
#   ./qa/context_slim/run.sh carve       # a `stream.*` subscriber can carve `stream.reasoning` out
#   ./qa/context_slim/run.sh ingress     # a large tool result reaches the model as marker + callable footer; ctx_search returns its body
#   ./qa/context_slim/run.sh breakdown   # after that turn, context.breakdown reports messages + tool_output
#   ./qa/context_slim/run.sh gate        # a result between DEFAULT_RESULT_BUDGET_TOKENS and MAX_RESULT_BUDGET_TOKENS is offloaded
#   ./qa/context_slim/run.sh firstparty  # Anthropic 1P vs a custom host: reasoning copy + context_management on the wire
#
#   KEEP=1 ./qa/context_slim/run.sh replay   # keep the scratch dir for post-mortem
#   SKIP_BUILD=1 …                           # use the already-built aleph-server
#   REQUIRE_FIELD=reasoning_details …        # break-it switch for `replay`: the
#                                            # mock's 400 rule checks a field the
#                                            # code never sends, so replay must FAIL
#
# ## How a local mock sits behind a first-party hostname
#
# Aleph classifies hosts by NAME: `api.deepseek.com` is DeepSeek-native (replay
# every turn's reasoning_content), `api.anthropic.com` is first-party (no
# reasoning_content copy, context editing allowed), `127.0.0.1` is neither. So
# `replay`, `carve` and the 1P arm of `firstparty` configure the provider with
# `base_url = "http://api.<vendor>.com"` and start the server with `HTTP_PROXY`
# pointing at the mock: reqwest sends the absolute-form request to the proxy,
# and the mock answers it. Nothing leaves the machine. `NO_PROXY` keeps the
# gateway's own loopback traffic direct.
#
# ## What this fixture does NOT cover
#
# The real vendors. The mock enforces DeepSeek's documented 400 rule, and
# encodes Anthropic's wire as Aleph's adapter reads it — it cannot prove the
# vendors accept what Aleph sends (U1: no live-provider probes).
set -uo pipefail

PHASE="${1:-replay}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
PLANH="$HERE/../plan_handoff"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-ctxslim-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18841}"
MOCK_PORT="${MOCK_PORT:-18842}"

case "$PHASE" in
  replay|carve|ingress|breakdown|gate|firstparty) ;;
  *) echo "unknown phase: $PHASE (replay|carve|ingress|breakdown|gate|firstparty)" >&2; exit 64 ;;
esac

. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
DB="$ALEPH_HOME/data/sessions.db"
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

SERVER_PID=""
MOCK_PID=""
say() { printf '\n=== %s ===\n' "$*"; }
stop_all() {
  for pid in "$SERVER_PID" "$MOCK_PID"; do [ -n "$pid" ] && kill "$pid" 2>/dev/null; done
  sleep 1
  for pid in "$SERVER_PID" "$MOCK_PID"; do [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null; done
  SERVER_PID=""; MOCK_PID=""
}
cleanup() {
  stop_all
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build ($PHASE)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  qa_build --bin aleph-server || { echo "build failed" >&2; exit 1; }
fi
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }
echo "binary: $BIN ($(date -r "$BIN" '+%Y-%m-%d %H:%M:%S'))"

say "generate a baseline config"
timeout 25 "$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }
cp "$CONFIG" "$QA_ROOT/config.baseline.toml"

# One arm: patch the provider, start the mock and the server, drive, stop.
#   arm NAME SCENARIO PROTOCOL BASE_URL MODEL PROXY(0|1) [--server-context-editing]
arm() {
  local name="$1" scenario="$2" protocol="$3" base_url="$4" model="$5" proxy="$6"
  shift 6
  local dir="$QA_ROOT/$name"
  mkdir -p "$dir"
  # Per-arm state: the session store and the file log (where tracing writes —
  # stdout carries only the banner), so one arm cannot read another's lines.
  rm -rf "$ALEPH_HOME/data" "$ALEPH_HOME/logs"
  cp "$QA_ROOT/config.baseline.toml" "$CONFIG"

  say "[$name] patch config"
  python3 "$BUSY/patch_config.py" "$CONFIG" \
    --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" --max-pending-steering 8 || return 1
  python3 "$HERE/patch_provider.py" "$CONFIG" --protocol "$protocol" \
    --base-url "$base_url" --model "$model" "$@" || return 1
  # `bash` is not idempotent, so the default tier would park the run on a
  # confirmation nobody answers. An explicit allow is the operator's own knob.
  python3 "$PLANH/add_overrides.py" "$CONFIG" bash=allow || return 1

  say "[$name] start mock ($scenario)"
  python3 "$HERE/mock_wire.py" "$MOCK_PORT" "$scenario" "$dir/requests.jsonl" \
    --require-field "${REQUIRE_FIELD:-reasoning_content}" >"$dir/mock.log" 2>&1 &
  MOCK_PID=$!
  sleep 1

  # The seatbelt profile lets the `bash` tool exec only system binaries: with
  # Homebrew first on PATH, `bash` resolves to /opt/homebrew/bin/bash and the
  # tool exits 71 (`execvp … Operation not permitted`) before it prints a byte.
  local sys_path="/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
  say "[$name] start server (proxy=$proxy)"
  if [ "$proxy" = "1" ]; then
    HTTP_PROXY="http://127.0.0.1:$MOCK_PORT" http_proxy="http://127.0.0.1:$MOCK_PORT" \
      NO_PROXY="127.0.0.1,localhost" no_proxy="127.0.0.1,localhost" PATH="$sys_path" \
      "$BIN" start >"$dir/server.log" 2>&1 &
  else
    PATH="$sys_path" "$BIN" start >"$dir/server.log" 2>&1 &
  fi
  SERVER_PID=$!
  for _ in $(seq 1 90); do
    curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && break
    kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$dir/server.log"; return 1; }
    sleep 1
  done
  echo "gateway up on $GATEWAY_PORT"
  # A server with no usable provider key runs `Mode: Simulated`, where tool
  # calls are placeholders — every downstream result would be meaningless.
  local mode
  mode="$(grep -m1 -o 'Mode: [A-Za-z]*' "$dir/server.log" || true)"
  echo "server ${mode:-Mode: (not logged)}"
  if echo "$mode" | grep -q Simulated; then
    echo "  [FAIL] the server runs in Simulated mode — no provider key reached it"
    return 1
  fi

  say "[$name] drive"
  local drive_phase="$PHASE"
  local arm_label=""
  [ "$PHASE" = "firstparty" ] && arm_label="$name"
  python3 "$HERE/drive.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$DB" "$drive_phase" \
    "$dir/requests.jsonl" "$arm_label" "$ALEPH_HOME/logs"
  local rc=$?
  echo "--- mock log (tail) ---"; tail -8 "$dir/mock.log"
  stop_all
  return "$rc"
}

RC=0
case "$PHASE" in
  replay|carve)
    arm "$PHASE" replay openai "http://api.deepseek.com" deepseek-reasoner 1 || RC=1
    ;;
  ingress)
    arm "$PHASE" ingress anthropic "http://127.0.0.1:$MOCK_PORT" claude-sonnet-4-6 0 || RC=1
    ;;
  breakdown)
    arm "$PHASE" ingress anthropic "http://127.0.0.1:$MOCK_PORT" claude-sonnet-4-6 0 \
      --context-budget || RC=1
    ;;
  gate)
    arm "$PHASE" gate anthropic "http://127.0.0.1:$MOCK_PORT" claude-sonnet-4-6 0 || RC=1
    ;;
  firstparty)
    arm 1p firstparty anthropic "http://api.anthropic.com" claude-sonnet-4-6 1 \
      --server-context-editing || RC=1
    arm custom firstparty anthropic "http://127.0.0.1:$MOCK_PORT" claude-sonnet-4-6 0 \
      --server-context-editing || RC=1
    ;;
esac

say "$PHASE: $([ "$RC" = 0 ] && echo PASS || echo FAIL)"
exit "$RC"
