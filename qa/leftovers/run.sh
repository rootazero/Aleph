#!/usr/bin/env bash
# Real-machine QA for the leftovers round.
#
#   ./qa/leftovers/run.sh
#
# Everything lands in a scratch HOME/ALEPH_HOME under $QA_ROOT, so this never
# touches the developer's ~/.aleph (two processes on one vault is a documented
# way to lose vault data — PROCESS_MANAGEMENT.md).
#
# No mock provider: none of the three claim groups runs an agent turn. Tools
# are driven directly through `tools.invoke`, which is exactly the surface an
# operator reaches, and the server never dials out.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-leftovers-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18797}"
MOCK_PORT="${MOCK_PORT:-18998}"   # nothing listens; the config just must not name a real provider
AGENT_ID="qa-leftover-agent"

# Build BEFORE HOME is redirected: cargo's registry, git cache and rustup
# toolchain all live under the real HOME, and a build launched with the scratch
# one silently degrades into a full network fetch that then times out.
. "$HERE/../lib/scratch_home.sh"
# Redirects HOME/ALEPH_HOME into the scratch root AND pins RUSTUP_HOME/
# CARGO_HOME at the real ones — the redirect and the pin are inseparable
# on purpose; see that file for the 1.3 GB-per-run leak it closes.
qa_redirect_home "$QA_ROOT"
export REAL_HOME   # this fixture drives child processes that need it
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"

# Deliberately outside $ALEPH_HOME as well as off the default layout: a root
# that merely sits elsewhere *inside* the home would still pass a sloppy
# `starts_with($ALEPH_HOME)` check.
AGENTS_ROOT="$QA_ROOT/elsewhere/agent-state"
WORKSPACE_ROOT="$QA_ROOT/somewhere-else/ws"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

SERVER_PID=""
say() { printf '\n=== %s ===\n' "$*"; }
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
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
python3 "$BUSY/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1
python3 "$HERE/patch_defaults.py" "$CONFIG" \
  --agents-root "$AGENTS_ROOT" --workspace-root "$WORKSPACE_ROOT" || exit 1
python3 - "$CONFIG" <<'PY' || exit 1
import sys, tomllib
d = tomllib.load(open(sys.argv[1], "rb"))
# A config the server refuses to parse still prints a startup banner, so a
# parse check here is what keeps "server died" from reading like a port clash.
print("config parses; [agents.defaults] =", d.get("agents", {}).get("defaults", {}))
PY

# `agent_create` is on the static `DANGEROUS_TOOLS` denylist for the
# `tools.invoke` surface, which has no approval transport. The product's own
# documented per-tool opt-in re-permits exactly one name — not a test-only
# bypass, and it does not touch the provisioning roots this run is about.
export ALEPH_GATEWAY_TOOLS_ALLOW="agent_create"

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

say "drive"
RC=0
python3 "$HERE/drive_leftovers.py" \
  "ws://127.0.0.1:$GATEWAY_PORT/ws" "$ALEPH_HOME" "$AGENTS_ROOT" "$WORKSPACE_ROOT" "$AGENT_ID" || RC=$?

say "provisioned tree"
find "$QA_ROOT/elsewhere" "$QA_ROOT/somewhere-else" -maxdepth 3 2>/dev/null | sed "s|$QA_ROOT|\$QA_ROOT|" | head -20
echo "--- default layout (must not contain $AGENT_ID) ---"
find "$ALEPH_HOME/agents" "$ALEPH_HOME/workspaces" -maxdepth 1 2>/dev/null | sed "s|$QA_ROOT|\$QA_ROOT|" | head -10

say "verdict: rc=$RC"
exit "$RC"
