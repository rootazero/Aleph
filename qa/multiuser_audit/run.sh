#!/usr/bin/env bash
# Real-machine QA for the multi-user round-6 work.
#
#   ./qa/multiuser_audit/run.sh
#
# Three claims, none of which any unit test can make:
#
#   1. The security audit trail is READABLE. Five producers had been writing to
#      `security_audit_log` with no reader anywhere; this drives the whole path
#      an operator actually walks — `aleph audit` over the wire, through the
#      admin gate, out of SQLite.
#   2. `users.update` tells the operator what the write DID. The receipt was
#      measured server-side and discarded by the only client, which printed a
#      hard-coded sentence in its place.
#   3. Revoking a device credential leaves an authority-change record naming
#      whose credential it was — a producer the `AuthorityChange` doc listed and
#      never had.
#
# Everything lands in a scratch HOME/ALEPH_HOME under $QA_ROOT: two processes on
# one vault is a documented way to lose vault data (PROCESS_MANAGEMENT.md).
#
# No mock provider and no agent turn — every verb here is a gateway RPC.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-mu6-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18801}"
MOCK_PORT="${MOCK_PORT:-18902}"   # nothing listens; the config just must not name a real provider
DEVICE_ID="qa-panel-mu6"

# Build BEFORE HOME is redirected: cargo's registry, git cache and rustup
# toolchain all live under the real HOME.
. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
export REAL_HOME
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

SERVER_PID=""
PASS=0
FAIL=0
say() { printf '\n=== %s ===\n' "$*"; }
ok()   { PASS=$((PASS+1)); printf 'PASS  %s\n' "$*"; }
bad()  { FAIL=$((FAIL+1)); printf 'FAIL  %s\n' "$*"; }
# Assert on captured output. `want` absent is a failure that prints the haystack
# — "the string was not there" and "the command produced nothing" must not read
# the same.
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
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  # Two invocations: `aleph` lives in the `aleph-cli` package, which is not in
  # the workspace's default-run set, so a bare `--bin aleph` resolves to nothing.
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
al() { "$CLI" --server "$URL" "$@" 2>&1; }

say "generate a baseline config"
timeout 25 "$SERVER" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }
python3 "$BUSY/patch_config.py" "$CONFIG" --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1

# The device half needs a NON-loopback peer: `resolve_connect_auth` authorises a
# loopback connection on its first line, before it reads `bootstrap_ticket`, so
# a ticket redeemed over 127.0.0.1 creates no device row — successfully and
# silently. Binding 0.0.0.0 for the length of this run is what makes the real
# pairing path reachable. The server has no provider configured and listens on a
# scratch port; it dies with the fixture.
python3 - "$CONFIG" <<'PY' || exit 1
import re, sys
p = sys.argv[1]
s = open(p, encoding="utf-8").read()
if re.search(r'(?m)^\s*host\s*=', s):
    s = re.sub(r'(?m)^(\s*)host\s*=.*$', r'\1host = "0.0.0.0"', s, count=1)
else:
    s = re.sub(r'(?m)^\[gateway\]\s*$', '[gateway]\nhost = "0.0.0.0"', s, count=1)
# The server refuses to serve plaintext off loopback — a fail-closed boot gate,
# and it is right. This is its own documented opt-in, not a way around it: the
# alternative (generating a self-signed cert and teaching two Python clients and
# the CLI to trust it) would test the TLS stack, which is not what this fixture
# is about. The exposure is a scratch server with no provider, no vault content
# and a random port, for the lifetime of one run.
s = re.sub(r'(?m)^\[gateway\]\s*$', '[gateway]\nallow_insecure_remote = true', s, count=1)
open(p, "w", encoding="utf-8").write(s)
print("gateway bound to 0.0.0.0 (plaintext opt-in) for the remote-pairing half")
PY

# A UDP "connect" to a public address picks the interface the kernel would route
# through without sending a packet — no DNS, no traffic, works offline.
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
say "1. the trail is readable at all"
# Before anything else happens: an empty window must still say the horizon it
# was purged against, otherwise "quiet" and "deleted" render identically.
OUT="$(al audit --type authority_change)"
expect "empty window names the retention horizon" "30 day(s)" "$OUT"
expect "empty window says it is not proof"        "not proof nothing happened" "$OUT"

# --------------------------------------------------------------------------
say "2. creating a principal is recorded, and the CLI can read it back"
OUT="$(al users create "QA Alice" --role member)"
expect "create reports the new principal" "Created QA Alice (member) as u-" "$OUT"
ALICE="$(printf '%s' "$OUT" | sed -n 's/.*as \(u-[0-9a-f-]*\).*/\1/p' | head -1)"
[ -n "$ALICE" ] && ok "captured principal id $ALICE" || { bad "could not parse the new user id"; }

OUT="$(al audit --type authority_change)"
expect "the create left an authority record" "users.create: created $ALICE role=member" "$OUT"
expect "the record names who did it"         "u-owner" "$OUT"

# The filter must actually narrow, or "it found my row" proves nothing.
OUT="$(al audit --type scoped_content_read)"
refute "an unrelated filter does not return the create" "users.create" "$OUT"

# --------------------------------------------------------------------------
say "3. pair a device to her, then check the receipt counts it"
PAIRED=0
if [ -z "$REMOTE_URL" ]; then
  # Not a pass. An assertion that could not run and one that succeeded must
  # never render the same — that is the whole failure mode of a silent skip.
  SKIPPED=$((${SKIPPED:-0}+1))
  printf 'SKIP  device pairing: no non-loopback address on this host\n'
else
  if python3 "$HERE/pair_device.py" "$URL" "$REMOTE_URL" "$ALICE" "$DEVICE_ID"; then
    ok "device paired and bound over $REMOTE_URL"; PAIRED=1
  else
    bad "device pairing driver failed"
  fi
fi

OUT="$(al audit --type authority_change)"
expect "minting the ticket was recorded" "gateway.ticket.create: bound to $ALICE" "$OUT"

OUT="$(al users update "$ALICE" --status deactivated)"
# The claim this round fixes: the CLI used to print one hard-coded sentence and
# threw the measured receipt away.
if [ "$PAIRED" = "1" ]; then
  expect "receipt counts the revoked device" "1 device revoked" "$OUT"
else
  expect "receipt reports the measured zero" "No devices were bound to them" "$OUT"
fi
expect "receipt reports the frozen legs"   "no running goals, loops or crons" "$OUT"
refute "no hard-coded plural claim survives" "Their devices are revoked and" "$OUT"

# --------------------------------------------------------------------------
say "4. revoking the credential names whose it was"
OUT="$(al audit --type authority_change)"
if [ "$PAIRED" = "1" ]; then
  expect "the device revoke is recorded"  "devices.revoke: $DEVICE_ID" "$OUT"
  expect "and it names the principal"     "(principal $ALICE)" "$OUT"
else
  refute "no device revoke is claimed when none was paired" "devices.revoke:" "$OUT"
fi
expect "the status transition is recorded" "users.update: status $ALICE →deactivated" "$OUT"

# --------------------------------------------------------------------------
say "5. reactivation says what did NOT come back"
OUT="$(al users update "$ALICE" --status active)"
expect "reactivation is qualified"        "did NOT restore" "$OUT"
expect "and names the device recovery verb" "pair --user" "$OUT"

OUT="$(al audit --type authority_change --actor u-owner)"
expect "the actor filter still finds the reactivation" "deactivated→active" "$OUT"

OUT="$(al audit --actor "$ALICE")"
refute "an actor who acted on nothing has no rows" "users.update" "$OUT"

# --------------------------------------------------------------------------
say "6. paging is honest about stopping"
OUT="$(al audit --limit 1)"
expect "a capped page says there is more" "More entries matched" "$OUT"
OUT="$(al audit --since 7w)"
expect "a bad --since unit is refused, not narrowed" "unrecognised --since unit" "$OUT"

say "verdict: $PASS passed, $FAIL failed, ${SKIPPED:-0} skipped"
[ "$FAIL" -eq 0 ] || exit 1
