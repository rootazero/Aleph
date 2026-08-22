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

# --------------------------------------------------------------------------
say "3. a member's spend lands on the MEMBER's row"
# The failure this whole round is about. A loopback peer is authorised before
# `resolve_connect_auth` reads `bootstrap_ticket`, so a second principal only
# exists over a non-loopback URL — the ticket must be redeemed there.
MEMBER=""
TICKET=""
if [ -z "$REMOTE_URL" ]; then
  skip "member identity: no non-loopback address on this host"
else
  OUT="$(al users create "QA Bob" --role member)"
  MEMBER="$(printf '%s' "$OUT" | sed -n 's/.*as \(u-[0-9a-f-]*\).*/\1/p' | head -1)"
  if [ -z "$MEMBER" ]; then
    bad "could not create a member principal"
    printf '%s\n' "$OUT" | sed 's/^/      | /'
  else
    TICKET="$(python3 - "$URL" "$MEMBER" <<'PY'
import asyncio, json, sys, websockets
async def main(url, uid):
    async with websockets.connect(url) as ws:
        await ws.send(json.dumps({"jsonrpc":"2.0","id":1,"method":"connect","params":{"client_type":"cli"}}))
        while True:
            if json.loads(await ws.recv()).get("id") == 1: break
        await ws.send(json.dumps({"jsonrpc":"2.0","id":2,"method":"gateway.ticket.create","params":{"user_id":uid}}))
        while True:
            m = json.loads(await ws.recv())
            if m.get("id") == 2:
                print((m.get("result") or {}).get("ticket",""))
                return
asyncio.run(main(sys.argv[1], sys.argv[2]))
PY
)"
    if [ -z "$TICKET" ]; then
      bad "could not mint a bootstrap ticket for $MEMBER"
    else
      BEFORE="$(ledger_rows | grep "^$MEMBER|" | awk -F'|' '{print $2}' | head -1)"
      BEFORE="${BEFORE:-0}"
      OUT="$(python3 "$DRIVE" "$REMOTE_URL" run gui:qa-spend-3 --ticket "$TICKET" --device qa-spend-bob)"
      expect "the member's run succeeds" '"ok": true' "$OUT"
      ROWS="$(ledger_rows)"
      AFTER="$(printf '%s\n' "$ROWS" | grep "^$MEMBER|" | awk -F'|' '{print $2}' | head -1)"
      AFTER="${AFTER:-0}"
      if python3 -c "import sys;sys.exit(0 if float('$AFTER')>float('$BEFORE') else 1)" 2>/dev/null; then
        ok "the member's own row grew ($BEFORE -> $AFTER) — spend is attributed to the person who spent it"
      else
        bad "the member's spend did not land on the member's row (this is the defect the round exists to fix)"
        printf '%s\n' "$ROWS" | sed 's/^/      | /'
      fi
      # An `@unattributed` row here would mean the run's principal could not be
      # resolved — a pass on the row above plus this row would be two different
      # runs, not one working one.
      refute "the member's run did not fall through to @unattributed" "@unattributed" "$(printf '%s\n' "$ROWS" | grep -c '^@unattributed|' | grep -v '^0$' && echo '@unattributed')"
    fi
  fi
fi

# --------------------------------------------------------------------------
say "4. past the per-user ceiling, the run is refused and the receipt names the reset"
stop_server
set_ceiling per_user 0.0000001
start_server || exit 1
OUT="$(python3 "$DRIVE" "$URL" run gui:qa-spend-4)"
expect "the run is refused"                       '"code": "SPEND_EXHAUSTED"' "$OUT"
expect "the refusal names when it resets"         "重置" "$OUT"
expect "the refusal names the caller's own spend" '上限' "$OUT"

# --------------------------------------------------------------------------
say "5. the same refusal in English goes through i18n, not a hardcoded string"
OUT="$(python3 "$DRIVE" "$URL" run gui:qa-spend-5 --language en)"
expect "still refused"          '"code": "SPEND_EXHAUSTED"' "$OUT"
expect "and refused in English" "Resets at" "$OUT"
refute "with no Chinese left over" "重置" "$OUT"

# --------------------------------------------------------------------------
say "6. the machine-total refusal names no numbers"
# `Limit::Total` is fieldless on purpose: `user_receipt` takes no actor, so
# there is no point at which "may this person see the machine total?" could be
# answered. G12, proven here on the wire rather than in a unit test.
stop_server
set_ceiling total 0.0000001
start_server || exit 1
OUT="$(python3 "$DRIVE" "$URL" run gui:qa-spend-6)"
expect "the run is refused on the machine total" '"code": "SPEND_EXHAUSTED"' "$OUT"
expect "and says so"                             "共享支出额度" "$OUT"
MSG="$(printf '%s' "$OUT" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("message",""))')"
if printf '%s' "$MSG" | grep -qE '\$[0-9]'; then
  bad "the machine-total refusal leaked a dollar figure: $MSG"
else
  ok "the machine-total refusal carries no dollar figure"
fi

# --------------------------------------------------------------------------
say "7. raising the ceiling applies live — no restart"
# The escape hatch. A person locked out by his own ceiling cannot raise it if
# raising it needs a restart, and "restart the server to unblock yourself" is
# how a budget becomes an outage. G14, end to end, against a server that is
# currently refusing this exact caller.
stop_server
set_ceiling per_user 0.0000001
start_server || exit 1
OUT="$(python3 "$DRIVE" "$URL" run gui:qa-spend-7a)"
expect "precondition: the caller is locked out" '"code": "SPEND_EXHAUSTED"' "$OUT"
PATCHED="$(python3 "$DRIVE" "$URL" patch policies.spend.per_user_usd 1000.0)"
expect "config.patch reports the write landed"  '"ok": true' "$PATCHED"
expect "and reports it applied live"            "Live" "$PATCHED"
OUT="$(python3 "$DRIVE" "$URL" run gui:qa-spend-7b)"
expect "the very next run succeeds, same process" '"ok": true' "$OUT"

# --------------------------------------------------------------------------
say "8. spend.query is admin-gated, and refused as NOT PERMITTED"
# "Refused" and "not found" are different failures and only one of them is the
# admin gate working — a no-oracle refusal here would be indistinguishable from
# the method not existing.
if [ -z "$TICKET" ]; then
  skip "member spend.query: no member identity on this host"
else
  Q="$(python3 "$DRIVE" "$REMOTE_URL" query --ticket "$TICKET" --device qa-spend-bob)"
  expect "the member is refused" '"error"' "$Q"
  if printf '%s' "$Q" | grep -qiE 'permission|permitted|admin|forbidden|denied'; then
    ok "refused as not permitted, not as not found"
  else
    bad "the refusal does not say it is a permission problem"
    printf '%s\n' "$Q" | sed 's/^/      | /'
  fi
  refute "and not as a missing method" "not found" "$Q"
fi

# --------------------------------------------------------------------------
say "9. aleph spend prints a row per principal with no dashes"
# `providers list` printed `type`/`default` while the server sent
# `provider_type`/`is_default`, so every row rendered a dash from the day it was
# written — and a dash reads as "no value yet", never as a bug.
OUT="$(al spend)"
expect "the command runs at all" "Principal" "$OUT"
BODY="$(printf '%s\n' "$OUT" | grep -E '^(u-|@unattributed)' || true)"
if [ -z "$BODY" ]; then
  bad "aleph spend printed no principal rows"
  printf '%s\n' "$OUT" | sed 's/^/      | /'
else
  if printf '%s\n' "$BODY" | grep -qE '(^|[[:space:]])-([[:space:]]|$)'; then
    bad "a rendered column is a dash — the column name does not match a wire field"
    printf '%s\n' "$BODY" | sed 's/^/      | /'
  else
    ok "every column on every row carries a real value"
  fi
fi

# --------------------------------------------------------------------------
say "10. spend survives a restart"
# The whole point of a durable ledger. An in-memory fallback passes every other
# assertion in this file and fails only this one.
BEFORE="$(ledger_rows | sort)"
Q_BEFORE="$(python3 "$DRIVE" "$URL" query)"
stop_server
start_server || exit 1
AFTER="$(ledger_rows | sort)"
Q_AFTER="$(python3 "$DRIVE" "$URL" query)"
if [ "$BEFORE" = "$AFTER" ] && [ -n "$BEFORE" ]; then
  ok "the ledger table is byte-identical across a real stop/start"
else
  bad "the ledger changed across a restart (or was empty to begin with)"
  printf 'before:\n%s\nafter:\n%s\n' "$BEFORE" "$AFTER" | sed 's/^/      | /'
fi
B_USD="$(printf '%s' "$Q_BEFORE" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(sum(r["usd"] for r in d.get("rows",[])))' 2>/dev/null)"
A_USD="$(printf '%s' "$Q_AFTER" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(sum(r["usd"] for r in d.get("rows",[])))' 2>/dev/null)"
if [ -n "$B_USD" ] && [ "$B_USD" = "$A_USD" ]; then
  ok "spend.query reports the same total after the restart ($A_USD)"
else
  bad "spend.query disagrees across the restart: $B_USD -> $A_USD"
fi

# --------------------------------------------------------------------------
say "11. an unpriced model counts, moves no dollars, and never denies"
# G3/G4 on a live box: a missing price is never a gate. The ceiling here is
# deliberately tiny — if an unpriced call moved any dollars at all it would
# cross it, so "no denial" and "usd unchanged" are the same assertion seen from
# two sides.
stop_server
set_ceiling per_user 0.01
start_server || exit 1
BEFORE_ROWS="$(ledger_rows)"
B_UNPRICED="$(printf '%s\n' "$BEFORE_ROWS" | grep -v '^@unattributed|' | awk -F'|' '{print $3}' | head -1)"
B_USD="$(printf '%s\n' "$BEFORE_ROWS" | grep -v '^@unattributed|' | awk -F'|' '{print $2}' | head -1)"
B_UNPRICED="${B_UNPRICED:-0}"; B_USD="${B_USD:-0}"
OUT="$(python3 "$DRIVE" "$URL" run gui:qa-spend-11 --agent unpriced)"
expect "the unpriced run is not denied" '"ok": true' "$OUT"
AFTER_ROWS="$(ledger_rows)"
A_UNPRICED="$(printf '%s\n' "$AFTER_ROWS" | grep -v '^@unattributed|' | awk -F'|' '{print $3}' | head -1)"
A_USD="$(printf '%s\n' "$AFTER_ROWS" | grep -v '^@unattributed|' | awk -F'|' '{print $2}' | head -1)"
A_UNPRICED="${A_UNPRICED:-0}"; A_USD="${A_USD:-0}"
if [ "$A_UNPRICED" -gt "$B_UNPRICED" ] 2>/dev/null; then
  ok "unpriced_calls incremented ($B_UNPRICED -> $A_UNPRICED)"
else
  bad "unpriced_calls did not increment ($B_UNPRICED -> $A_UNPRICED)"
  printf '%s\n' "$AFTER_ROWS" | sed 's/^/      | /'
fi
if python3 -c "import sys;sys.exit(0 if abs(float('$A_USD')-float('$B_USD'))<1e-12 else 1)" 2>/dev/null; then
  ok "usd is untouched by an unpriced call ($A_USD)"
else
  bad "an unpriced call moved dollars: $B_USD -> $A_USD"
fi

# --------------------------------------------------------------------------
printf '\n=== %d passed, %d failed, %d skipped ===\n' "$PASS" "$FAIL" "$SKIPPED"
[ "$FAIL" -eq 0 ]
