#!/usr/bin/env bash
# Real-machine QA for the MCP server face (spec §3.7 / §6).
#
#   ./qa/mcp_face/run.sh handshake     # three versions negotiate; unsupported → newest;
#                                      # sessions minted / 400 / 404; 202 for notifications
#   ./qa/mcp_face/run.sh tools         # tools/list ⊆ expose on the REAL registry; a read-only
#                                      # call answers text, isError:false
#   ./qa/mcp_face/run.sh auth          # bound to 0.0.0.0: LAN request 401 without / with a
#                                      # wrong bearer; the shared gateway token is admitted
#                                      # (SKIP when the host has no non-loopback address)
#   ./qa/mcp_face/run.sh list_changed  # plugin_manage disable/enable → SSE list_changed +
#                                      # the plugin's tool leaves/re-enters tools/list
#   ./qa/mcp_face/run.sh deny          # unexposed tool → -32602; a confirmation-gated tool with
#                                      # NO operator surface → isError:true naming the Panel
#
# Why real-machine: the unit tests drive a stub registry through the router
# in-process. Only a booted daemon can show that the face was actually
# INSTALLED (a slot boot never fills is a 404 with nothing red), that the
# expose list survives `config.toml`, that the real registry's schemas arrive,
# and that the attendance probe reads the real connection table.
#
# Everything lands in a scratch HOME/ALEPH_HOME under $QA_ROOT (two processes
# on one vault is the documented way to lose vault data — PROCESS_MANAGEMENT.md).
set -uo pipefail

STAGE="${1:-handshake}"
case "$STAGE" in handshake|tools|auth|list_changed|deny) ;; *)
  echo "unknown stage '$STAGE' (handshake|tools|auth|list_changed|deny)" >&2; exit 64;;
esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-mcp-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18831}"
MOCK_PORT="${MOCK_PORT:-18832}"   # nothing listens; the config must merely not name a real provider

# Build BEFORE HOME is redirected (cargo's caches live under the real HOME).
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
export REAL_HOME
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
INSTALLED="$ALEPH_HOME/plugins/installed"
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

SERVER_PID=""
say() { printf '\n=== %s ===\n' "$*"; }
stop_server() {
  [ -n "$SERVER_PID" ] || return 0
  kill "$SERVER_PID" 2>/dev/null
  for _ in $(seq 1 30); do kill -0 "$SERVER_PID" 2>/dev/null || break; sleep 0.5; done
  kill -9 "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
}
cleanup() {
  stop_server
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! qa_build -p alephcore --bin aleph-server; then
    echo "build failed" >&2; exit 1
  fi
fi
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

say "generate a baseline config"
# `--port` on the GENERATION boot: without it this boot binds the built-in
# default port and dies if anything already holds it (see qa/plugins/run.sh).
timeout 25 "$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
python3 "$BUSY/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1

# The exposure under test. ONE spelling, shared with the driver, so
# `tools/list ⊆ expose` is asserted against the same list the server read.
EXPOSE="agent_list,agent_info,grep,find,file_read,session_list,skill_list"
LAN_FLAG=""
case "$STAGE" in
  list_changed) EXPOSE="$EXPOSE,qa_mcp_probe" ;;
  deny)         EXPOSE="$EXPOSE,agent_delete" ;;   # confirmation-gated (CONFIRMATION_REQUIRED_TOOLS)
  auth)         LAN_FLAG="--lan" ;;
esac
python3 "$HERE/patch_mcp.py" "$CONFIG" --expose "$EXPOSE" $LAN_FLAG || exit 1

if [ "$STAGE" = "list_changed" ]; then
  say "plant a static plugin with one [[tools]] entry"
  mkdir -p "$INSTALLED/qa-mcp-probe"
  cat >"$INSTALLED/qa-mcp-probe/aleph.plugin.toml" <<'TOML'
[plugin]
id = "qa-mcp-probe"
name = "QA MCP probe"
version = "0.0.1"
kind = "static"
entry = "SKILL.md"

[[tools]]
name = "qa_mcp_probe"
description = "a tool whose only job is to appear and disappear in tools/list"
handler = "probe"
TOML
  printf '# QA probe\n' >"$INSTALLED/qa-mcp-probe/SKILL.md"
fi

LAN_IP=""
if [ "$STAGE" = "auth" ]; then
  # A UDP "connect" picks the interface the kernel would route through
  # without sending a packet (same trick as qa/spend_budget/run.sh).
  LAN_IP="$(python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
try:
    s.connect(("8.8.8.8", 80)); ip = s.getsockname()[0]
except OSError:
    ip = ""
finally:
    s.close()
print("" if ip.startswith("127.") else ip)
PY
)"
  [ -n "$LAN_IP" ] || echo "no non-loopback address on this host; the remote assertions will report SKIP" >&2
fi

say "start server"
# stdout is not a TTY here, so tracing goes to $ALEPH_HOME/logs/; the
# redirect below catches only the startup banner.
"$BIN" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 90); do
  curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && break
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

say "the face was INSTALLED (not merely compiled)"
# The banner line is printed by start/mod.rs only on the install arm; a
# declined face (simulated mode, enabled=false) prints nothing here and every
# stage below would 404 with nothing else red.
if grep -q "MCP server face: /mcp" "$QA_ROOT/server.log"; then
  echo "  [PASS] boot installed the MCP face"
else
  echo "  [FAIL] boot did not install the MCP face (Mode: Simulated? enabled=false?)"
  grep -n "Mode:" "$QA_ROOT/server.log" | head -3
  exit 1
fi
if grep -q "does not know at boot" "$ALEPH_HOME"/logs/*.log 2>/dev/null; then
  echo "  [WARN] boot reported an unknown expose name:"; grep -h "does not know at boot" "$ALEPH_HOME"/logs/*.log | head -3
fi

say "drive: $STAGE"
RC=0
python3 -u "$HERE/drive.py" "$STAGE" "$GATEWAY_PORT" "$EXPOSE" "$LAN_IP" || RC=$?

say "server log tail"
tail -5 "$QA_ROOT/server.log"
exit "$RC"
