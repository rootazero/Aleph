#!/usr/bin/env bash
# Real-machine QA for binding a channel group conversation to a project room.
#
#   ./qa/rooms_channel_bind/run.sh
#   KEEP=1 ./qa/rooms_channel_bind/run.sh      # keep the scratch dir
#   HOLD=1 ./qa/rooms_channel_bind/run.sh      # leave the gateway up for a browser
#   SKIP_BUILD=1 ./qa/rooms_channel_bind/run.sh
#
# Everything this branch claims — the `projects.channel.*` handlers, the CLI,
# the Panel section, the roster gate on arm 2, `rescope_attribution` — has so
# far rested on compile-and-unit-test evidence. Nothing had spoken to a live
# gateway. This is the first thing that does.
#
# ## What it exercises, and why each one needs a real machine
#
#   1. An unbound group files each speaker's turn under their OWN partition.
#      This is the behaviour the round is motivated by; it is asserted FIRST so
#      a regression in the premise fails loudly instead of making the later
#      scenarios look correct.
#   2. `aleph projects channel bind` — a real CLI process against a real
#      gateway — upgrades the next turn to the room's partition AND moves the
#      conversation's existing row, which a roster member then sees in
#      `sessions.list`.
#   3/4. A paired non-member and an unpaired stranger each stay out of the
#      room's partition. Both negative assertions run only after a positive
#      query on the same partition has passed (Ruling AG).
#   5. `<room_context>` reaches the model on a CHANNEL turn, names a member who
#      has never spoken, and survives one `subagent` spawn into the child's own
#      prompt.
#   6. `unbind` stops future turns and keeps what is already filed.
#   7. An `agent_switch` mints a new session key and the room survives it —
#      which is the entire reason the binding table is keyed on the
#      conversation rather than on the session key.
#   8/8b. Ruling AQ evidence: two different doors onto a bound-but-silent
#      conversation, with the stored rows quoted verbatim. Evidence, not a
#      verdict — nothing here changes `request_scope` or
#      `ensure_session_under_request_scope`.
#   A/B/C/E. The four things the batch-10/11/12 review could only assert
#      statically: the tier gate against a real chat-tier connection, both new
#      clients over a live wire, a genuine store failure driving
#      `RescopeOutcome::Unknown`, and a non-admin's write refusal.
#
# Addendum D (does the Panel section survive a narrow viewport) is a BROWSER
# claim and no shell assertion can make it. `HOLD=1` exists for exactly that:
# it parks the gateway with all of the above already built so a browser can be
# pointed at it. If nobody points one, the honest report says the section was
# not looked at — a surface nobody rendered is a different state from one that
# rendered correctly.
#
# ## The three identities are not optional
#
# `resolve_connect_auth` authorises a loopback peer on its first line, before
# it reads `bootstrap_ticket` — so a ticket redeemed over 127.0.0.1 binds no
# principal, silently and successfully. Every member here connects over this
# host's LAN address, which is what `allow_insecure_remote` in the patched
# config is for. Everything runs under a scratch HOME/ALEPH_HOME: two processes
# on one vault is a documented way to lose vault data (PROCESS_MANAGEMENT.md).
#
# ## Why Node
#
# The gateway's only client transport is a WebSocket, and on a Windows host
# there is no usable Python at all (the only `python3` on PATH is the
# WindowsApps stub). Node ships a WHATWG WebSocket client, an HTTP server, an
# HMAC, and — since v22 — `node:sqlite`, which is what lets this fixture read
# the rows on disk rather than asking the server what it thinks it wrote.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18817}"
MOCK_PORT="${MOCK_PORT:-18917}"
WEBHOOK_SECRET="${WEBHOOK_SECRET:-qa-rooms-channel-bind-secret}"

command -v node >/dev/null 2>&1 || { echo "node is required for this fixture" >&2; exit 1; }
node -e 'require("node:sqlite")' >/dev/null 2>&1 || {
  echo "this fixture reads the rows on disk and needs node:sqlite (node >= 22)" >&2; exit 1; }

# `qa_build` is called by the hoisted block below, so build.sh has to be sourced
# above it — not down next to `scratch_home.sh`, where the HOME redirect needs
# its own helper.
. "$HERE/../lib/build.sh"

# --- build BEFORE the HOME redirect ----------------------------------------
# Deliberately ahead of `qa_redirect_home`: the per-command `HOME="$REAL_HOME"
# cargo …` guard the sibling fixtures use is correct on POSIX, where the pinned
# RUSTUP_HOME/CARGO_HOME are POSIX paths cargo understands. On Windows those
# pins are msys paths (`/c/Users/…`) the native toolchain cannot read, so the
# only safe place for a cargo invocation is before any of it happens. Nothing
# after this line runs cargo.
#
# The CLI is built too, and that is not incidental: addendum B's whole point is
# that `aleph projects channel bind` had never spoken to a live gateway.
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
ALEPH_CLI="$TARGET_DIR/debug/aleph"
[ -x "$ALEPH_CLI" ] || ALEPH_CLI="$ALEPH_CLI.exe"
[ -x "$ALEPH_CLI" ] || { echo "no aleph CLI binary under $TARGET_DIR/debug" >&2; exit 1; }

# --- scratch root ----------------------------------------------------------
# On Windows the root is kept in mixed form (`C:/…`) rather than the msys form
# (`/c/…`): bash accepts both, the native `aleph-server` accepts only the
# first, and a `/c/…` ALEPH_HOME resolves against the current drive root
# instead — silently, into a tree the fixture would then fail to clean up.
if [ -z "${QA_ROOT:-}" ]; then
  QA_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-bind-XXXXXX")"
  command -v cygpath >/dev/null 2>&1 && QA_ROOT="$(cygpath -m "$QA_ROOT")"
fi

. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
export REAL_HOME
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
WORKSPACE="$QA_ROOT/workspace"
REQUEST_LOG="$QA_ROOT/requests.jsonl"
OUTBOUND_LOG="$QA_ROOT/outbound.jsonl"
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

mkdir -p "$WORKSPACE"
printf 'QA workspace for the channel-binding fixture.\n' > "$WORKSPACE/README.md"
: > "$REQUEST_LOG"
: > "$OUTBOUND_LOG"

SERVER_PID=""
MOCK_PID=""
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

echo "=== generate a baseline config ==="
timeout 40 "$SERVER" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 60); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -30 "$QA_ROOT/gen.log"; exit 1; }
node "$HERE/patch_config.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" "$WEBHOOK_SECRET" || exit 1

echo "=== start the mock provider + outbound sink ==="
node "$HERE/mock_llm.mjs" "$MOCK_PORT" "$REQUEST_LOG" "$OUTBOUND_LOG" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 40); do
  curl -s -o /dev/null "http://127.0.0.1:$MOCK_PORT/v1/models" && break
  sleep 0.25
done

echo "=== start the server ==="
"$SERVER" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
UP=0
for _ in $(seq 1 120); do
  if curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null; then UP=1; break; fi
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
# "I could not ask" must never render as "the answer is no".
[ "$UP" = "1" ] || { echo "HARNESS_GATEWAY_NEVER_CAME_UP"; tail -40 "$QA_ROOT/server.log"; exit 1; }
echo "gateway up on $GATEWAY_PORT"

# The channel has to have STARTED, not merely been configured: a webhook whose
# handler was never mounted answers every inbound POST with 404, and a fixture
# that reads that as "the message did not produce a run" would blame the wrong
# half of the system.
MOUNTED=0
for _ in $(seq 1 40); do
  code="$(curl -s -o /dev/null -w '%{http_code}' -X POST \
    -H 'content-type: application/json' -d '{}' \
    "http://127.0.0.1:$GATEWAY_PORT/webhook/qa" 2>/dev/null)"
  # 403 = mounted and the signature was (correctly) rejected. 404 = not mounted.
  if [ "$code" = "403" ]; then MOUNTED=1; break; fi
  sleep 1
done
[ "$MOUNTED" = "1" ] || {
  echo "HARNESS_WEBHOOK_NEVER_MOUNTED (last status ${code:-none})"
  grep -i webhook "$QA_ROOT/server.log" | tail -20
  exit 1
}
echo "webhook channel mounted on /webhook/qa"

echo "=== drive ==="
node "$HERE/drive_bind.mjs" \
  "$GATEWAY_PORT" "$MOCK_PORT" "$WEBHOOK_SECRET" \
  "$REQUEST_LOG" "$OUTBOUND_LOG" "$ALEPH_CLI" "$ALEPH_HOME" "$WORKSPACE"
RC=$?

if [ "$RC" != "0" ]; then
  echo
  echo "--- server log tail ---"
  tail -80 "$QA_ROOT/server.log"
  echo "--- mock log tail ---"
  tail -40 "$QA_ROOT/mock.log"
fi

# HOLD=1 — park the gateway after the assertions so a BROWSER can be pointed at
# the state they just built. Every assertion above is an RPC round trip or a
# row on disk, so it proves the SERVER answers correctly and says nothing about
# whether the Panel's room-settings page renders the channel section at all —
# and that page has never been looked at (addendum D).
#
# Loopback is always operator and credential-free, so a browser on this machine
# needs no ticket; that is also why the LAN-leg member checks above use a real
# credential and this does not.
if [ "${HOLD:-0}" = "1" ]; then
  echo
  echo "=== HOLD: gateway is still up ==="
  echo "  Panel:   http://127.0.0.1:$GATEWAY_PORT/"
  echo "  Room:    \"QA Bound Room\"  (settings -> channel bindings)"
  echo "  Expect -> a Channels section listing webhook:qa-c1 labelled \"QA C1 Group\","
  echo "            plus qa-c2 / qa-c3, with bind + unbind controls VISIBLE"
  echo "            (they gate on role, not ownership — so a non-admin sees them)"
  echo "  Check  -> narrow (<=420px) and wide viewports; the section must stay"
  echo "            reachable and readable in both."
  echo "  scratch: $QA_ROOT"
  echo "  Ctrl-C to tear down."
  # `wait` alone would return as soon as ANY child settles; this parks until the
  # server itself exits or the trap fires.
  while kill -0 "$SERVER_PID" 2>/dev/null; do sleep 2; done
fi

exit "$RC"
