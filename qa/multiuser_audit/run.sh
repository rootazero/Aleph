#!/usr/bin/env bash
# Real-machine QA for the multi-user round-6 work.
#
#   ./qa/multiuser_audit/run.sh
#
# The claims below, none of which any unit test can make. There is deliberately
# no count in this sentence: it read "three claims" while the list had grown a
# fourth, which is the cheapest possible instance of the shape this fixture
# exists to catch.
#
#   1. The security audit trail is READABLE. Five producers had been writing to
#      `security_audit_log` with no reader anywhere; this drives the whole path
#      an operator actually walks — `aleph audit` over the wire, through the
#      admin gate, out of SQLite.
#   2. `users.update` tells the operator what the write DID. The receipt was
#      measured server-side and discarded by the only client, which printed a
#      hard-coded sentence in its place. This also covers the one background
#      leg the freeze CANNOT measure here: the patcher turns the heartbeat
#      service off, so the receipt has to say the leg did not run rather than
#      report a zero — a boot-time decline arm, on the wire, out of the CLI.
#   3. Revoking a device credential leaves an authority-change record naming
#      whose credential it was — a producer the `AuthorityChange` doc listed and
#      never had.
#   4. `users.get` composes the SAME join BEFORE the irreversible write. Until
#      it existed, the only way to learn what a principal held was to
#      deactivate them and read claim 2's receipt. Stage 3b asserts the preview
#      while she is still active, including the declined heartbeat leg, which
#      must read as "not counted" and never as a zero.
#
# Everything lands in a scratch HOME/ALEPH_HOME under $QA_ROOT: two processes on
# one vault is a documented way to lose vault data (PROCESS_MANAGEMENT.md).
#
# No mock provider and no agent turn — every verb here is a gateway RPC.
#
# ## Why Node and not Python
#
# This fixture had five `python3` legs. On a Windows host the only `python3` on
# PATH is the WindowsApps stub: `python3 - <<'PY'` prints nothing and edits
# nothing, so the config rewrite silently did not happen and the run died far
# from its cause. (Its exit code is stub-version dependent and deliberately not
# recorded here; the silence is the operative half.) Its two sibling multi-user fixtures (`teamchat_rooms`,
# `rooms_channel_bind`) already do all of this from Node, so this is reuse and
# not a second toolchain: the config patcher IS `qa/teamchat_rooms/
# patch_config.mjs` (the same file `qa/agents_viz/run.sh` calls) and the pairing
# driver is a `.mjs` next to this script. Not every fixture could follow —
# `qa/spend_budget` keeps a much larger Python surface (`spend_rpc.py`,
# `mock_anthropic.py`, float comparisons, a `jf` helper) and does not run on a
# host without a real python3; `qa/README.md` says so on its entry rather than
# leaving it to be discovered.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18801}"
MOCK_PORT="${MOCK_PORT:-18902}"   # nothing listens; the config just must not name a real provider
DEVICE_ID="qa-panel-mu6"

command -v node >/dev/null 2>&1 || { echo "node is required for this fixture" >&2; exit 1; }

# `qa_build` is called by the hoisted block below, so build.sh has to be sourced
# above it — not down next to `scratch_home.sh`, where the HOME redirect needs
# its own helper.
. "$HERE/../lib/build.sh"

# --- build BEFORE the HOME redirect ----------------------------------------
# Deliberately ahead of `qa_redirect_home`: the per-command `HOME="$REAL_HOME"
# cargo …` guard this fixture used to carry is correct on POSIX, where the
# pinned RUSTUP_HOME/CARGO_HOME are POSIX paths cargo understands. On Windows
# those pins are msys paths (`/c/Users/…`) the native toolchain cannot read, so
# rustup concludes the pinned toolchain is missing and starts downloading a
# fresh one; the fixture then sits in `=== build ===` until something kills it,
# which reads like a slow compile. Nothing after this line runs cargo.
#
# Two invocations: `aleph` lives in the `aleph-cli` package, which is not in the
# workspace's default-run set, so a bare `--bin aleph` resolves to nothing.
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  echo "=== build (server + cli) ==="
  qa_build --bin aleph-server || { echo "server build failed" >&2; exit 1; }
  qa_build -p aleph-cli --bin aleph || { echo "cli build failed" >&2; exit 1; }
fi
TARGET_DIR="$(cd "$REPO" && cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | node -e 'let s="";process.stdin.on("data",c=>s+=c).on("end",()=>console.log(JSON.parse(s).target_directory))')"
SERVER="$TARGET_DIR/debug/aleph-server"
[ -x "$SERVER" ] || SERVER="$SERVER.exe"
[ -x "$SERVER" ] || { echo "no server binary under $TARGET_DIR/debug" >&2; exit 1; }
CLI="$TARGET_DIR/debug/aleph"
[ -x "$CLI" ] || CLI="$CLI.exe"
[ -x "$CLI" ] || { echo "no aleph CLI binary under $TARGET_DIR/debug" >&2; exit 1; }

# --- scratch root ----------------------------------------------------------
# On Windows the root is kept in mixed form (`C:/…`) rather than the msys form
# (`/c/…`): bash accepts both, the native `aleph-server` accepts only the
# first, and a `/c/…` ALEPH_HOME resolves against the current drive root
# instead — silently, into a tree the fixture would then fail to clean up.
if [ -z "${QA_ROOT:-}" ]; then
  QA_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-mu6-XXXXXX")"
  command -v cygpath >/dev/null 2>&1 && QA_ROOT="$(cygpath -m "$QA_ROOT")"
fi

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

URL="ws://127.0.0.1:$GATEWAY_PORT/ws"
al() { "$CLI" --server "$URL" "$@" 2>&1; }

say "generate a baseline config"
# `--port` on the GENERATION boot. The config does not exist yet, so without
# it this boot binds the built-in default port — and if anything already holds
# that port (another fixture, a dev server, the operator's own daemon) the
# process exits before writing a config at all. The symptom is
# `no config generated at …`, which reads like a permissions or path problem;
# the cause is one line further up the log. Binding the port this run already
# owns makes the generation boot as isolated as the real one.
timeout 25 "$SERVER" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

# The shared patcher makes the daemon inert (one mock provider nothing listens
# on, no channels) AND opens the LAN leg this fixture's device half needs, for
# the same reason `teamchat_rooms` does: `resolve_connect_auth` authorises a
# loopback connection on its first line, before it reads `bootstrap_ticket`, so
# a ticket redeemed over 127.0.0.1 creates no device row — successfully and
# silently. `allow_insecure_remote` is the server's own documented opt-in, not
# a way around it (the alternative, a self-signed cert plus clients taught to
# trust it, would test the TLS stack, which is not what this fixture is about);
# the exposure is a scratch server with no provider, no vault content and a
# scratch port, for the lifetime of one run. It leaves `[memory]` ON where this
# fixture's previous patcher had it off — nothing here writes or reads a note,
# and a second patcher differing in one key is exactly the duplicate this port
# exists not to create.
node "$HERE/../teamchat_rooms/patch_config.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" || exit 1

# A UDP "connect" to a public address picks the interface the kernel would route
# through without sending a packet — no DNS, no traffic, works offline. An empty
# answer is the only honest one when there is no non-loopback address; the
# caller turns it into a SKIP, never into a pass.
LAN_IP="$(node -e '
const dgram = require("node:dgram");
const s = dgram.createSocket("udp4");
const done = (ip) => {
  try { s.close(); } catch { /* never opened */ }
  console.log(ip.startsWith("127.") ? "" : ip);
  process.exit(0);
};
s.on("error", () => done(""));
s.connect(80, "8.8.8.8", () => done(s.address().address));
')"
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
  if node "$HERE/pair_device.mjs" "$URL" "$REMOTE_URL" "$ALICE" "$DEVICE_ID"; then
    ok "device paired and bound over $REMOTE_URL"; PAIRED=1
  else
    bad "device pairing driver failed"
  fi
fi

OUT="$(al audit --type authority_change)"
expect "minting the ticket was recorded" "gateway.ticket.create: bound to $ALICE" "$OUT"

# --------------------------------------------------------------------------
say "3b. the dossier is readable BEFORE the one-way door"
# The claim: this join — devices, spend, background work, rooms, all for one
# principal — used to exist ONLY as the receipt of the deactivation below, i.e.
# only after the write it should have informed. Every assertion here is made
# while ALICE is still active, and every one of them has a counterpart in the
# receipt further down; that pairing is the point.
OUT="$(al users show "$ALICE")"
expect "the dossier names the principal"    "($ALICE)"          "$OUT"
expect "and their role"                     "role:     member"  "$OUT"
expect "and their status, still active"     "status:   active"  "$OUT"
if [ "$PAIRED" = "1" ]; then
  expect "and counts her live panel device" "1 live panel device"   "$OUT"
else
  expect "and says she holds none"          "no live panel devices" "$OUT"
fi
expect "and reports her rooms"              "rooms:    none" "$OUT"
# Fail-closed (criterion #8) on the wire and out of the CLI: nothing was spent,
# and "nothing recorded" must not be rendered as a measured 0.00 — that is the
# figure an operator would act on.
expect "an unrecorded spend is a sentence"  "no spend recorded this period" "$OUT"
refute "and never a dollar figure"          "0.00"                          "$OUT"
# The DECLINED heartbeat arm again, on the read side. This fixture's patcher
# sets `[heartbeat] enabled = false`, so the preview must make exactly the
# distinction the receipt below makes: a leg nobody measured is not a leg that
# found nothing. A preview that folded it into `0` would tell the operator this
# principal owns no heartbeat task while their tasks were still armed.
expect "an unmeasured heartbeat leg says so" "Heartbeat tasks were NOT counted" "$OUT"
refute "and never a fabricated count"        "heartbeat task(s)"                "$OUT"
# Admin-only, no carve-out, no Panel face (OI-63): the verb reached the server
# and came back rendered, which is the half a unit test cannot claim. The cost
# statement is asserted family by family because a two-of-four list reads as
# coverage — the operator would decide without learning the channel binding
# and the outstanding pairing ticket die too.
expect "and warns what deactivation would cost" "None of that is undone by reactivating" "$OUT"
expect "  naming the burned tickets"            "outstanding bootstrap tickets"          "$OUT"
expect "  and the withdrawn channel senders"    "channel sender approvals"               "$OUT"

OUT="$(al users update "$ALICE" --status deactivated)"
# The claim this round fixes: the CLI used to print one hard-coded sentence and
# threw the measured receipt away.
if [ "$PAIRED" = "1" ]; then
  expect "receipt counts the revoked device" "1 device revoked" "$OUT"
else
  expect "receipt reports the measured zero" "No devices were bound to them" "$OUT"
fi
expect "receipt reports the frozen legs"   "no running goals, loops or crons" "$OUT"
# The bootstrap-ticket leg, on the wire and out of the CLI. The ticket minted
# above was redeemed by the pairing driver, so the honest count here is zero —
# and zero is exactly what proves the whole path: only a deactivation carries
# this field, so the sentence appearing at all means the server measured the
# leg, the field crossed the wire, and the renderer fired. Drop either half and
# this line disappears.
expect "receipt reports the ticket leg"    "outstanding bootstrap tickets were left" "$OUT"
refute "no hard-coded plural claim survives" "Their devices are revoked and" "$OUT"
# The heartbeat leg, and specifically its DECLINED arm — the one thing here no
# unit test can claim, because it starts in `aleph-server start`'s
# `[heartbeat] enabled = false` branch (this fixture's patcher sets exactly
# that, alongside cron), travels as an absent field on the wire, and has to
# come out of the CLI as a sentence. Before this leg existed the receipt named
# three of four subsystems and read as a complete inventory.
expect "an unrun heartbeat leg says so" "Heartbeat tasks were NOT checked" "$OUT"
expect "and says what is still running" "still armed" "$OUT"
# A leg that did not run must never render as a measured zero, in either
# spelling: neither a count nor an inventory that silently includes it.
refute "no fabricated heartbeat count"  "heartbeat task(s)" "$OUT"
refute "no four-leg claim from a three-leg freeze" "loops, crons or heartbeat" "$OUT"

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
