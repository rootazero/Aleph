#!/usr/bin/env bash
# Real-machine QA for channel reachability — feishu / line / qq, with msteams
# as the control — and for the Lark failure paths behind them.
#
#   ./qa/channels/run.sh            # all three phases
#   ./qa/channels/run.sh reach      # phase 1 only (reachability)
#   ./qa/channels/run.sh errors     # phase 2 only (throttle / refusal)
#   ./qa/channels/run.sh approval   # phase 3 only (notify-and-wait, channel leg)
#
# Phase 2 needs phase 1's server anyway (a channel that never started cannot be
# throttled), so those two modes select which assertions run, not which server
# boots. Phase 3 is different: it needs a tool behind the confirmation gate, and
# `policies` is `ReloadImpact::Restart`, so it gets its OWN boot with its own
# config. Under `all` that is a second boot after phases 1-2, not a patch.
#
# This replaces item 18 of qa/picker_nav's manual checklist. That item was the
# only end-to-end evidence that the feishu CONNECT of 2026-08-18 works, and it
# lived as a paragraph a human had to read and obey — which is how it rotted
# once already (its first assertion looked for a `Failed to create channel`
# line that this path never prints).
#
# Everything lands in a scratch HOME/ALEPH_HOME under $QA_ROOT, so this never
# touches the developer's ~/.aleph (two processes on one vault is a documented
# way to lose vault data — PROCESS_MANAGEMENT.md).
#
# Nothing dials the internet: the provider is qa/busy_input/mock_anthropic.py
# and the Feishu Open Platform is qa/channels/mock_lark.py.
set -uo pipefail

MODE="${1:-all}"
case "$MODE" in
  all|reach|errors|approval) ;;
  *) echo "usage: $0 [all|reach|errors|approval]" >&2; exit 64 ;;
esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-channels-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18801}"
MOCK_PORT="${MOCK_PORT:-18802}"        # anthropic-protocol provider stub
LARK_PORT="${LARK_PORT:-18803}"        # Feishu Open Platform stub
FEISHU_HOOK_PORT="${FEISHU_HOOK_PORT:-18804}"
FEISHU_HOOK_PATH="/feishu/events"
FEISHU_TOKEN="qa-feishu-verification-token"

# Build BEFORE HOME is redirected: cargo's registry, git cache and rustup
# toolchain all live under the real HOME, and a build launched with the scratch
# one silently degrades into a full network fetch that then times out.
. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
OBS="$QA_ROOT/lark-observations.jsonl"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

SERVER_PID=""; MOCK_PID=""; LARK_PID=""
say() { printf '\n=== %s ===\n' "$*"; }
cleanup() {
  for pid in "$SERVER_PID" "$MOCK_PID" "$LARK_PID"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null
  done
  sleep 1
  for pid in "$SERVER_PID" "$MOCK_PID" "$LARK_PID"; do
    [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null
  done
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! (cd "$REPO" && HOME="$REAL_HOME" cargo build --bin aleph-server 2>&1 | tail -5); then
    echo "build failed" >&2; exit 1
  fi
fi
BIN="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps \
        | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/debug/aleph-server"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

say "generate a baseline config"
# `--port` even though this boot is thrown away: without it the generation pass
# binds the DEFAULT 18790 and dies with "Address already in use" on any machine
# where Aleph is actually running — i.e. every developer's. The scratch
# ALEPH_HOME isolates the singleton lock but not the port, so the config file
# never gets written and every phase below fails on a missing config.
timeout 25 "$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

# The tool phase 3 puts behind the confirmation gate. It is the one the mock
# provider already calls, so arming the gate needs no change to the provider
# fixture — only a permission override.
GATED_TOOL="file_read"

patch_config() {  # $1: extra args
  python3 "$HERE/patch_config.py" "$CONFIG" \
    --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" --lark-port "$LARK_PORT" \
    --feishu-webhook-port "$FEISHU_HOOK_PORT" --feishu-webhook-path "$FEISHU_HOOK_PATH" \
    --feishu-token "$FEISHU_TOKEN" $1 || exit 1
}

say "patch config (4 channels: feishu real-start, line+qq registration, msteams control)"
# Booting straight into the gated config when phase 3 is the only phase: an
# unconditional plain boot would mean two boots for a one-phase run, and the
# first of them would be answering assertions nobody asked for.
if [ "$MODE" = "approval" ]; then
  patch_config "--ask-tool $GATED_TOOL"
else
  patch_config ""
fi

say "start mock Lark + mock provider"
python3 "$HERE/mock_lark.py" "$LARK_PORT" "$OBS" >"$QA_ROOT/lark.log" 2>&1 &
LARK_PID=$!
python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" /etc/hostname quick >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
sleep 1

# stdout is not a TTY here, so tracing goes to $ALEPH_HOME/logs/ — this file
# catches only the startup banner, which is where the `Registered channel:`
# and `✓ Channel … started` lines live.
start_server() {  # $1: log file
  "$BIN" start >"$1" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 90); do
    curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && break
    kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$1"; exit 1; }
    sleep 1
  done
  # A config the server could not parse is answered by falling back to
  # DEFAULTS — including the default port — so the next symptom is an
  # unrelated "address already in use" and the real fault never gets named.
  # Fail on the parse, not on its second-order consequence.
  if grep -q "Failed to parse config" "$1"; then
    echo "the server could not parse the patched config:"
    grep -A 6 "Failed to parse config" "$1"
    exit 1
  fi
  echo "gateway up on $GATEWAY_PORT"
  # The channel start_all() pass runs after the health endpoint is live.
  sleep 3
}

stop_server() {
  [ -n "$SERVER_PID" ] || return 0
  kill "$SERVER_PID" 2>/dev/null
  # The singleton lock (~/.aleph/data/aleph.lock) is held for the process
  # lifetime, so the next boot fails outright if this one is still alive.
  for _ in $(seq 1 30); do kill -0 "$SERVER_PID" 2>/dev/null || break; sleep 0.5; done
  kill -9 "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
}

say "start server"
start_server "$QA_ROOT/server.log"

RC=0

if [ "$MODE" != "errors" ] && [ "$MODE" != "approval" ]; then
  say "phase 1 — reachability"
  python3 "$HERE/drive_channels.py" \
    "$QA_ROOT/server.log" "$ALEPH_HOME/logs" \
    "http://127.0.0.1:$LARK_PORT" \
    "http://127.0.0.1:$FEISHU_HOOK_PORT$FEISHU_HOOK_PATH" \
    "$FEISHU_TOKEN" || RC=$((RC + $?))
fi

if [ "$MODE" != "reach" ] && [ "$MODE" != "approval" ]; then
  # Phase 2 runs second on purpose. Its cases all read "the channel called the
  # send endpoint N times", and that sentence means nothing until phase 1 has
  # established that the channel calls it at all — a dead channel and a channel
  # that gave up after one throttle produce the same count.
  say "phase 2 — Lark failure paths (throttle / refusal)"
  if [ "$MODE" = "errors" ]; then
    # Standing alone, phase 2 still needs one completed round trip first, so
    # that its per-case counts start from a channel known to be sending.
    python3 "$HERE/drive_channels.py" \
      "$QA_ROOT/server.log" "$ALEPH_HOME/logs" \
      "http://127.0.0.1:$LARK_PORT" \
      "http://127.0.0.1:$FEISHU_HOOK_PORT$FEISHU_HOOK_PATH" \
      "$FEISHU_TOKEN" >/dev/null 2>&1 || true
  fi
  python3 "$HERE/drive_lark_errors.py" \
    "http://127.0.0.1:$LARK_PORT" \
    "http://127.0.0.1:$FEISHU_HOOK_PORT$FEISHU_HOOK_PATH" \
    "$FEISHU_TOKEN" "$ALEPH_HOME/logs" || RC=$((RC + $?))
fi

if [ "$MODE" = "all" ] || [ "$MODE" = "approval" ]; then
  if [ "$MODE" = "all" ]; then
    # `policies` is ReloadImpact::Restart: a runtime config.patch would be
    # saved and ignored, and the phase would then assert against a gate that
    # was never armed. So the gate arrives by reboot.
    say "phase 3 setup — reboot with $GATED_TOOL behind the confirmation gate"
    stop_server
    patch_config "--ask-tool $GATED_TOOL"
    # The provider's turn counter is GLOBAL, not per run, and the `quick` plan
    # has three entries — by now every turn would answer `end` and never call a
    # tool at all. A phase asserting "a tool call parked" against a plan that
    # emits no tool calls is the vacuous-green shape, so the counter is reset
    # with the process.
    kill "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null
    python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" /etc/hostname quick \
      >"$QA_ROOT/mock3.log" 2>&1 &
    MOCK_PID=$!
    sleep 1
    start_server "$QA_ROOT/server3.log"
  fi

  say "phase 3 — notify-and-wait on the channel leg"
  python3 "$HERE/drive_approval.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" \
    "http://127.0.0.1:$LARK_PORT" \
    "http://127.0.0.1:$FEISHU_HOOK_PORT$FEISHU_HOOK_PATH" \
    "$FEISHU_TOKEN" "$ALEPH_HOME/logs" || RC=$((RC + $?))
fi

say "server banner"
grep -E "Registered channel|Channel .* (started|failed)" "$QA_ROOT/server.log" || true

say "verdict: rc=$RC"
exit "$RC"
