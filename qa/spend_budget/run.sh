#!/usr/bin/env bash
# Real-machine QA for the per-principal spend budget (round-7).
#
#   ./qa/spend_budget/run.sh
#
# Eleven assertions, each checking an EFFECT. Nothing here counts calls: the
# things this round can get wrong all render as a plausible success — a ceiling
# that never denies, a ledger that resets on restart, a report that says
# `configured: false` on a box whose config sets a ceiling.
#
# Two of the assertions are observed from OUTSIDE the process, by reading the
# `spend_ledger` table with sqlite3 rather than asking the handler. A handler
# that agrees with itself proves nothing about what was persisted.
#
# Everything lands in a scratch HOME/ALEPH_HOME under $QA_ROOT: two processes on
# one vault is a documented way to lose vault data (PROCESS_MANAGEMENT.md).
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-spend-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18811}"
MOCK_PORT="${MOCK_PORT:-18912}"

# Build BEFORE HOME is redirected: cargo's registry, git cache and rustup
# toolchain all live under the real HOME.
. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
export REAL_HOME
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

SERVER_PID=""
MOCK_PID=""
PASS=0
FAIL=0
SKIPPED=0
say() { printf '\n=== %s ===\n' "$*"; }
ok()   { PASS=$((PASS+1)); printf 'PASS  %s\n' "$*"; }
bad()  { FAIL=$((FAIL+1)); printf 'FAIL  %s\n' "$*"; }
skip() { SKIPPED=$((SKIPPED+1)); printf 'SKIP  %s\n' "$*"; }
# `want` absent is a failure that prints the haystack — "the string was not
# there" and "the command produced nothing" must not read the same.
expect() {
  local label="$1" want="$2" hay="$3"
  if printf '%s' "$hay" | grep -qF -- "$want"; then ok "$label"; else
    bad "$label (missing: $want)"
    printf '%s\n' "$hay" | sed 's/^/      | /' | head -20
  fi
}
refute() {
  local label="$1" unwanted="$2" hay="$3"
  if printf '%s' "$hay" | grep -qF -- "$unwanted"; then
    bad "$label (unexpectedly present: $unwanted)"
    printf '%s\n' "$hay" | sed 's/^/      | /' | head -20
  else ok "$label"; fi
}
cleanup() {
  for pid in "$SERVER_PID" "$MOCK_PID"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null
  done
  sleep 1
  for pid in "$SERVER_PID" "$MOCK_PID"; do
    [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null
  done
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! (cd "$REPO" && HOME="$REAL_HOME" cargo build --bin aleph-server 2>&1 | tail -3); then
    echo "server build failed" >&2; exit 1
  fi
  if ! (cd "$REPO" && HOME="$REAL_HOME" cargo build -p aleph-cli --bin aleph 2>&1 | tail -3); then
    echo "cli build failed" >&2; exit 1
  fi
fi
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
SERVER="$TARGET_DIR/debug/aleph-server"
CLI="$TARGET_DIR/debug/aleph"
[ -x "$SERVER" ] || { echo "no server binary at $SERVER" >&2; exit 1; }
[ -x "$CLI" ] || { echo "no cli binary at $CLI" >&2; exit 1; }
URL="ws://127.0.0.1:$GATEWAY_PORT/ws"
DRIVE="$HERE/drive_spend.py"
al() { "$CLI" --server "$URL" "$@" 2>&1; }

# The ledger lives in the one boot-opened SecurityStore. Finding its file is
# how assertions 1/2/10 observe persistence from outside the process.
ledger_rows() {
  local db
  db="$(find "$ALEPH_HOME" -name '*.db' -o -name '*.sqlite' 2>/dev/null | head -20)"
  for f in $db; do
    if sqlite3 "$f" "select name from sqlite_master where type='table' and name='spend_ledger'" 2>/dev/null | grep -q spend_ledger; then
      sqlite3 "$f" "select principal_id, usd, unpriced_calls, partial_calls from spend_ledger" 2>/dev/null
      return 0
    fi
  done
  echo "__NO_SPEND_LEDGER_TABLE__"
}

say "mock provider"
python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" "$QA_ROOT/probe" quick >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
sleep 1

say "generate a baseline config"
timeout 25 "$SERVER" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }
python3 "$BUSY/patch_config.py" "$CONFIG" --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1

# The agent's model id decides whether a call PRICES. `lookup_rates` falls back
# to `infer_vendor(model)`, so `claude-sonnet-4` under a provider called
# `qa-mock` resolves to anthropic's rates — a real dollar figure with a mock on
# the other end and no network. `qa-unpriced-*` infers no vendor and lands on
# `CostStatus::Unknown`, which is what assertion 11 needs.
PRICED_MODEL="claude-sonnet-4"
UNPRICED_MODEL="qa-unpriced-model"
python3 - "$CONFIG" "$PRICED_MODEL" "$UNPRICED_MODEL" <<'PY' || exit 1
import re, sys
path, priced, unpriced = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(path, encoding="utf-8").read()
s = s.replace('models = ["qa-mock-model"]', f'models = ["{priced}", "{unpriced}"]')
s = s.replace('model = "qa-mock-model"', f'model = "{priced}"')
# A second agent on the deliberately unpriced model — assertion 11 needs a run
# whose price the table cannot resolve, and switching an agent is the only way
# to change the model a `chat.send` actually uses without a per-turn override.
s += f'''
[[agents.list]]
id = "unpriced"
name = "QA Unpriced"
model = "{unpriced}"
provider = "qa-mock"
system_prompt = "QA fixture."
'''
# Remote pairing needs a non-loopback bind; see the round-6 fixture for the
# full argument. Scratch server, no provider credentials, random port.
if re.search(r'(?m)^\s*host\s*=', s):
    s = re.sub(r'(?m)^(\s*)host\s*=.*$', r'\1host = "0.0.0.0"', s, count=1)
else:
    s = re.sub(r'(?m)^\[gateway\]\s*$', '[gateway]\nhost = "0.0.0.0"', s, count=1)
s = re.sub(r'(?m)^\[gateway\]\s*$', '[gateway]\nallow_insecure_remote = true', s, count=1)
open(path, "w", encoding="utf-8").write(s)
print(f"agents: main->{priced}, unpriced->{unpriced}; gateway on 0.0.0.0")
PY

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
REMOTE_URL=""
[ -n "$LAN_IP" ] && REMOTE_URL="ws://$LAN_IP:$GATEWAY_PORT/ws"

start_server() {
  "$SERVER" start >>"$QA_ROOT/server.log" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 90); do
    curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && return 0
    kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; return 1; }
    sleep 1
  done
  echo "server never became healthy"; return 1
}
stop_server() {
  [ -n "$SERVER_PID" ] || return 0
  kill "$SERVER_PID" 2>/dev/null
  for _ in $(seq 1 30); do kill -0 "$SERVER_PID" 2>/dev/null || break; sleep 0.5; done
  kill -9 "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
}
set_ceiling() {  # set_ceiling <per_user|total|none> <value>
  python3 - "$CONFIG" "$1" "$2" <<'PY'
import re, sys
path, which, value = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(path, encoding="utf-8").read()
s = re.sub(r'(?ms)^\[policies\.spend\].*?(?=^\[|\Z)', '', s)
if which != "none":
    key = "per_user_usd" if which == "per_user" else "total_usd"
    s = s.rstrip() + f'\n\n[policies.spend]\n{key} = {value}\nperiod = "month"\n'
open(path, "w", encoding="utf-8").write(s)
PY
}

say "start server (no [policies.spend])"
set_ceiling none 0
start_server || exit 1

# --------------------------------------------------------------------------
say "1. an unconfigured box says so, and records nothing"
OUT="$(python3 "$DRIVE" "$URL" run gui:qa-spend-1)"
expect "a run on an unconfigured box succeeds" '"ok": true' "$OUT"
Q="$(python3 "$DRIVE" "$URL" query)"
expect "spend.query answers configured: false" '"configured": false' "$Q"
ROWS="$(ledger_rows)"
if [ "$ROWS" = "__NO_SPEND_LEDGER_TABLE__" ]; then
  bad "the spend_ledger table does not exist — the SQLite ledger was never installed at boot"
elif [ -z "$ROWS" ]; then
  ok "the ledger has zero rows (G8: a disabled policy never touches it), observed from outside the process"
else
  bad "a disabled policy wrote to the ledger anyway"
  printf '%s\n' "$ROWS" | sed 's/^/      | /'
fi

# --------------------------------------------------------------------------
say "2. with a ceiling, a run under it records real dollars"
stop_server
set_ceiling per_user 100.0
start_server || exit 1
OUT="$(python3 "$DRIVE" "$URL" run gui:qa-spend-2)"
expect "a run well under the ceiling succeeds" '"ok": true' "$OUT"
Q="$(python3 "$DRIVE" "$URL" query)"
expect "spend.query now answers configured: true" '"configured": true' "$Q"
ROWS="$(ledger_rows)"
OWNER_USD="$(printf '%s\n' "$ROWS" | grep -v '^@unattributed|' | awk -F'|' '{print $2}' | head -1)"
if [ -n "$OWNER_USD" ] && python3 -c "import sys;sys.exit(0 if float('$OWNER_USD')>0 else 1)" 2>/dev/null; then
  ok "a real principal's row carries a non-zero usd ($OWNER_USD), observed from outside the process"
else
  bad "no attributed row with non-zero usd after a priced run"
  printf '%s\n' "$ROWS" | sed 's/^/      | /'
fi
