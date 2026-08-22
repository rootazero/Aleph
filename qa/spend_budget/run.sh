#!/usr/bin/env bash
# Real-machine QA for the per-principal spend budget round (task 12).
#
#   ./qa/spend_budget/run.sh
#
# Eleven assertions, each reading an EFFECT (a ledger row on disk, a wire
# error code, a CLI table, a survived restart) rather than counting an RPC's
# "it returned 200". Two of them (1, 2) read `spend_ledger` with `sqlite3`
# directly, not through `spend.query` — that is what makes them evidence
# about the LEDGER rather than about the handler that reads it back.
#
# Built on `qa/multiuser_audit/` (round-6): scratch HOME via
# `qa/lib/scratch_home.sh::qa_redirect_home`, the loopback-mint / LAN-redeem
# device pattern, and the same PASS/FAIL/expect/refute harness. Uses
# `qa/busy_input/mock_anthropic.py` (the `single-shot` plan added for this
# fixture: every turn answers immediately with no tool call, so one
# `chat.send` is exactly one priced LLM call — no multi-turn floor-arm
# mid-run cutoff to disentangle from the admission arm's own denial).
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-spend-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18821}"
MOCK_PORT="${MOCK_PORT:-18922}"
DEVICE_ID="qa-spend-panel"

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
say() { printf '\n=== %s ===\n' "$*"; }
ok()   { PASS=$((PASS+1)); printf 'PASS  %s\n' "$*"; }
bad()  { FAIL=$((FAIL+1)); printf 'FAIL  %s\n' "$*"; }
# Assert on captured output. `want` absent is a failure that prints the
# haystack — "the string was not there" and "the command produced nothing"
# must not read the same.
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
# Numeric assertions on sqlite output: string matching on a float (which may
# render as `0.0`, `7e-05`, or `7.0000000000000007e-05` depending on the
# exact value) is exactly the kind of assertion that passes for the wrong
# reason — compare as floats instead.
expect_eq() {
  local label="$1" want="$2" got="$3"
  if python3 -c "import sys; sys.exit(0 if float('$got' or 'nan') == float('$want') else 1)" 2>/dev/null; then
    ok "$label"
  else
    bad "$label (want $want, got '$got')"
  fi
}
expect_gt() {
  local label="$1" got="$2" floor="$3"
  if python3 -c "import sys; sys.exit(0 if float('$got' or 'nan') > float('$floor') else 1)" 2>/dev/null; then
    ok "$label"
  else
    bad "$label (want > $floor, got '$got')"
  fi
}
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
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
LOCAL_URL="ws://127.0.0.1:$GATEWAY_PORT/ws"
al() { "$CLI" --server "$LOCAL_URL" "$@" 2>&1; }
rpc() { python3 -u "$HERE/spend_rpc.py" "$@"; }
# Pull one field out of a `spend_rpc.py` JSON line with a python one-liner —
# every subcommand prints exactly one JSON object as its last line.
jf() { python3 -c "import json,sys; print(json.load(sys.stdin)$1)"; }

say "generate a baseline config"
timeout 25 "$SERVER" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }
python3 "$HERE/patch_config.py" "$CONFIG" --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1

# A UDP "connect" to a public address picks the interface the kernel would
# route through without sending a packet — no DNS, no traffic, works
# offline. Same trick as `qa/multiuser_audit/run.sh`.
LAN_IP="$(python3 - <<'PY'
import socket
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
try:
    s.connect(("8.8.8.8", 80))
    ip = s.getsockname()[0]
except OSError:
    ip = ""
finally:
    s.close()
print("" if ip.startswith("127.") else ip)
PY
)"
REMOTE_URL=""
[ -n "$LAN_IP" ] && REMOTE_URL="ws://$LAN_IP:$GATEWAY_PORT/ws"
if [ -z "$REMOTE_URL" ]; then
  echo "no non-loopback address on this host; assertions 3 and 8 (member identity) cannot run" >&2
  echo "the fixture will report them SKIP rather than PASS" >&2
fi

say "start mock provider (single-shot plan)"
python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" /etc/hostname single-shot >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
sleep 1

say "start server"
"$SERVER" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 90); do
  curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && break
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

# --------------------------------------------------------------------------
say "1. no [policies.spend]: configured=false, and a real (priced) run writes NOTHING to the ledger"
DB="$ALEPH_HOME/data/security.db"
OUT="$(al spend)"
expect "the CLI states no ceiling is configured" "No spend ceiling is configured" "$OUT"

# A real run — priced model pinned via override — BEFORE any policy exists.
# G8: the disabled policy must not touch the ledger at all, not even to
# write a zero row.
R="$(rpc chat "$LOCAL_URL" "agent:main:main:qa-operator" "assertion 1: no policy yet" --model claude-haiku-4-5)"
OUTCOME="$(printf '%s' "$R" | jf "['outcome']")"
expect "the priced run completed while spend is unconfigured" "complete" "$OUTCOME"
ROWS="$(sqlite3 "$DB" "SELECT COUNT(*) FROM spend_ledger;" 2>/dev/null || echo "ERR")"
if [ "$ROWS" = "0" ]; then ok "spend_ledger has zero rows with no policy configured"; else
  bad "spend_ledger has zero rows with no policy configured (got $ROWS)"
fi

# --------------------------------------------------------------------------
say "2. enable a tiny per-user ceiling; a run under it succeeds with non-zero usd"
PATCHED="$(rpc patch "$LOCAL_URL" "policies.spend" '{"per_user_usd": 0.00001, "period": "month"}')"
expect "the policy patch applied" '"ok": true' "$PATCHED"

# assertion 11 first, deliberately: spend is still zero for this principal,
# so an UNPRICED call (the agent's default model) is admitted the same way
# a priced one would be — the admission arm only looks at ACCUMULATED
# spend, never at what the upcoming call might cost.
say "11. an unpriced model (default, no override) is metered as a COUNT, never denies, never moves usd"
R="$(rpc chat "$LOCAL_URL" "agent:main:main:qa-operator" "assertion 11: unpriced default model")"
OUTCOME="$(printf '%s' "$R" | jf "['outcome']")"
expect "the unpriced-model run completed (not denied)" "complete" "$OUTCOME"
USD_AFTER_UNPRICED="$(sqlite3 "$DB" "SELECT usd FROM spend_ledger WHERE principal_id='u-owner';" 2>/dev/null)"
UNPRICED_CALLS="$(sqlite3 "$DB" "SELECT unpriced_calls FROM spend_ledger WHERE principal_id='u-owner';" 2>/dev/null)"
expect_eq "the unpriced call left usd untouched (still zero)" 0 "$USD_AFTER_UNPRICED"
expect_eq "the unpriced call incremented unpriced_calls" 1 "$UNPRICED_CALLS"

R="$(rpc chat "$LOCAL_URL" "agent:main:main:qa-operator" "assertion 2: priced call under the ceiling" --model claude-haiku-4-5)"
OUTCOME="$(printf '%s' "$R" | jf "['outcome']")"
expect "the priced run under the ceiling completed" "complete" "$OUTCOME"
OWNER_USD_1="$(sqlite3 "$DB" "SELECT usd FROM spend_ledger WHERE principal_id='u-owner';" 2>/dev/null)"
expect_gt "the ledger's usd for u-owner is non-zero after one priced call" "$OWNER_USD_1" 0

# --------------------------------------------------------------------------
say "4/5. the SAME session, now over the ceiling: refused with SPEND_EXHAUSTED, naming the reset time, in English"
R="$(rpc chat "$LOCAL_URL" "agent:main:main:qa-operator" "assertion 4: should be denied" --model claude-haiku-4-5)"
OUTCOME="$(printf '%s' "$R" | jf "['outcome']")"
expect "the second call over the ceiling is refused, not silent" "error" "$OUTCOME"
CODE="$(printf '%s' "$R" | jf "['error_code']")"
expect "the refusal carries the stable SPEND_EXHAUSTED code" "SPEND_EXHAUSTED" "$CODE"
MSG="$(printf '%s' "$R" | jf "['error']")"
expect "the message names the reset time" "Resets at" "$MSG"
expect "the message is English (language=\"en\"), not the hardcoded Chinese arm" "Spend ceiling reached" "$MSG"
refute "the English arm is not Chinese" "已花费" "$MSG"

# --------------------------------------------------------------------------
say "7. raising per_user_usd via config.patch reports Live, and the next run succeeds — no restart"
PATCHED="$(rpc patch "$LOCAL_URL" "policies.spend" '{"per_user_usd": 100.0, "period": "month"}')"
expect "the raise reports a Live reload impact" '"kind": "live"' "$PATCHED"
R="$(rpc chat "$LOCAL_URL" "agent:main:main:qa-operator" "assertion 7: should succeed after the raise" --model claude-haiku-4-5)"
OUTCOME="$(printf '%s' "$R" | jf "['outcome']")"
expect "the next run succeeds immediately, without a restart" "complete" "$OUTCOME"
OWNER_USD_2="$(sqlite3 "$DB" "SELECT usd FROM spend_ledger WHERE principal_id='u-owner';" 2>/dev/null)"

# --------------------------------------------------------------------------
say "3/8. a member's spend lands on the MEMBER's row, not the operator's; spend.query refuses her (not-permitted, not not-found)"
MEMBER_SKIPPED=0
if [ -z "$REMOTE_URL" ]; then
  printf 'SKIP  assertion 3 (member priced call): no non-loopback address on this host\n'
  printf 'SKIP  assertion 8 (member spend.query refusal): no non-loopback address on this host\n'
  MEMBER_SKIPPED=1
else
  OUT="$(al users create "QA Spend Alice" --role member)"
  expect "the member principal was created" "Created QA Spend Alice (member) as u-" "$OUT"
  ALICE="$(printf '%s' "$OUT" | sed -n 's/.*as \(u-[0-9a-f-]*\).*/\1/p' | head -1)"
  if [ -z "$ALICE" ]; then
    bad "could not parse the new member's user id"
  else
    ok "captured member id $ALICE"
    MINTED="$(rpc mint_and_redeem "$LOCAL_URL" "$REMOTE_URL" "$ALICE" "$DEVICE_ID")"
    TOKEN="$(printf '%s' "$MINTED" | jf "['device_token']" 2>/dev/null)"
    if [ -z "$TOKEN" ] || [ "$TOKEN" = "None" ]; then
      bad "device pairing failed: $MINTED"
    else
      ok "device paired and bound to $ALICE"

      R="$(rpc chat "$REMOTE_URL" "agent:main:main:qa-member" "assertion 3: member's own priced call" --model claude-haiku-4-5 --device-token "$TOKEN")"
      OUTCOME="$(printf '%s' "$R" | jf "['outcome']")"
      expect "the member's priced run completed" "complete" "$OUTCOME"
      ALICE_ROW="$(sqlite3 "$DB" "SELECT usd FROM spend_ledger WHERE principal_id='$ALICE';" 2>/dev/null)"
      expect_gt "the member's spend landed on HER OWN row with a non-zero usd" "$ALICE_ROW" 0
      OWNER_USD_AFTER_MEMBER="$(sqlite3 "$DB" "SELECT usd FROM spend_ledger WHERE principal_id='u-owner';" 2>/dev/null)"
      expect_eq "the operator's OWN row is unaffected by the member's spend" "$OWNER_USD_2" "$OWNER_USD_AFTER_MEMBER"

      say "8. spend.query from the member is refused as NOT PERMITTED, not as not-found"
      R="$(rpc query "$REMOTE_URL" --device-token "$TOKEN")"
      OK_FIELD="$(printf '%s' "$R" | jf "['ok']")"
      expect "the member's spend.query call is refused" "False" "$OK_FIELD"
      MCODE="$(printf '%s' "$R" | jf "['code']")"
      expect "refused with the admin-gate code (AUTH_REQUIRED = -32000)" "-32000" "$MCODE"
      refute "NOT the visibility not-found code (-32009)" "-32009" "$MCODE"
      MMSG="$(printf '%s' "$R" | jf "['message']")"
      expect "the refusal names the operator-privilege reason, not \"not found\"" "operator privileges" "$MMSG"
      refute "the refusal does not read as a missing resource" "not found" "$MMSG"
    fi
  fi
fi

# --------------------------------------------------------------------------
say "6. a machine-total ceiling names NO dollar figures — Limit::Total is fieldless by construction"
PATCHED="$(rpc patch "$LOCAL_URL" "policies.spend" '{"total_usd": 0.00001, "per_user_usd": 100.0, "period": "month"}')"
expect "the total_usd patch applied live" '"kind": "live"' "$PATCHED"
R="$(rpc chat "$LOCAL_URL" "agent:main:main:qa-operator" "assertion 6: should hit the machine-total ceiling" --model claude-haiku-4-5)"
OUTCOME="$(printf '%s' "$R" | jf "['outcome']")"
expect "the call is refused by the machine-total ceiling" "error" "$OUTCOME"
CODE="$(printf '%s' "$R" | jf "['error_code']")"
expect "still the stable SPEND_EXHAUSTED code" "SPEND_EXHAUSTED" "$CODE"
MSG="$(printf '%s' "$R" | jf "['error']")"
refute "the Total refusal carries no dollar figure at all" '$' "$MSG"
expect "but it still names the reset time" "Resets at" "$MSG"

# --------------------------------------------------------------------------
say "9. aleph spend prints a row per principal with no dashes — every column carries a real value"
JSON_OUT="$(al --json spend)"
# Feed JSON_OUT through a small checker: every row must have a non-empty
# `principal` and numeric `usd`/`unpriced_calls`/`partial_calls` — the exact
# fields `render_row` renders, checked at the wire rather than trusted from
# reading `render_row`'s source.
CHECK="$(printf '%s' "$JSON_OUT" | python3 -c '
import json, sys
try:
    doc = json.load(sys.stdin)
except Exception as e:
    print(f"PARSE_ERROR:{e}")
    sys.exit(0)
rows = doc.get("rows", [])
if not rows:
    print("NO_ROWS")
    sys.exit(0)
problems = []
for r in rows:
    p = r.get("principal")
    if not isinstance(p, str) or not p:
        problems.append(f"empty principal in {r}")
    for f in ("usd", "unpriced_calls", "partial_calls"):
        if not isinstance(r.get(f), (int, float)):
            problems.append(f"missing/non-numeric {f} in {r}")
print("OK" if not problems else "PROBLEMS:" + "; ".join(problems))
print(f"ROW_COUNT:{len(rows)}")
')"
expect "every row's principal/usd/unpriced_calls/partial_calls is a real value" "OK" "$CHECK"
if [ "$MEMBER_SKIPPED" = "0" ]; then
  expect "at least two principals are listed (operator + member)" "ROW_COUNT:2" "$CHECK"
fi
TABLE_OUT="$(al spend)"
expect "the plain table names the operator" "u-owner" "$TABLE_OUT"
[ "$MEMBER_SKIPPED" = "0" ] && [ -n "${ALICE:-}" ] && expect "the plain table names the member" "$ALICE" "$TABLE_OUT"

# --------------------------------------------------------------------------
say "10. spend survives a server restart: stop, start, query, same numbers"
BEFORE="$(rpc query "$LOCAL_URL")"
BEFORE_ROWS="$(printf '%s' "$BEFORE" | jf "['result']['rows']")"
ok "captured pre-restart rows: $BEFORE_ROWS"

kill "$SERVER_PID" 2>/dev/null
sleep 1
kill -9 "$SERVER_PID" 2>/dev/null
wait "$SERVER_PID" 2>/dev/null
SERVER_PID=""

"$SERVER" start >"$QA_ROOT/server_restart.log" 2>&1 &
SERVER_PID=$!
RESTARTED=0
for _ in $(seq 1 90); do
  curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && { RESTARTED=1; break; }
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died on restart"; tail -60 "$QA_ROOT/server_restart.log"; break; }
  sleep 1
done
if [ "$RESTARTED" = "1" ]; then
  ok "server restarted and became healthy again"
  AFTER="$(rpc query "$LOCAL_URL")"
  AFTER_ROWS="$(printf '%s' "$AFTER" | jf "['result']['rows']")"
  expect "spend.query after restart reports configured=true" '"ok": true' "$AFTER"
  if [ "$BEFORE_ROWS" = "$AFTER_ROWS" ]; then
    ok "the ledger rows are byte-identical before and after the restart"
  else
    bad "the ledger rows changed across a restart"
    printf '  before: %s\n  after:  %s\n' "$BEFORE_ROWS" "$AFTER_ROWS"
  fi
else
  bad "server did not come back up after restart"
fi

say "verdict: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ] || exit 1
