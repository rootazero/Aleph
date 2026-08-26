#!/usr/bin/env bash
# Real-machine QA for the multi-user × team-chat × project-rooms round.
#
#   ./qa/teamchat_rooms/run.sh
#   KEEP=1 ./qa/teamchat_rooms/run.sh      # keep the scratch dir for post-mortem
#   SKIP_BUILD=1 ./qa/teamchat_rooms/run.sh
#
# Seven claims, none of which a unit test can make, because each one needs two
# authenticated principals and a live run between them:
#
#   1. A room-scoped team really is reachable by every member of the room —
#      created by a model inside a room run, then addressed by two different
#      humans over two different sockets.
#   2. The activation gate flips on the SECOND human: a plain message is
#      observed, an @-mention dispatches, and both are broadcast live.
#   3. The approval card a member run raises is addressable by the human who
#      spoke — and NOT by the room's other member.
#   4. `<room_context>` reaches the model with both members named, including
#      one who has never spoken, and the user turn is speaker-prefixed.
#   5. The name comes back out: the agent's reply addresses the speaker.
#   6. Each project-page tab has a server-side answer: teams.list carries the
#      room's scope stamp, the bound workspace lists and reads, a room run's
#      note lands in `main__p-<id>`, and `projects.changed` reaches both
#      sockets live.
#   7. A child the room's run DELEGATES inherits the room: its own prompt
#      carries `<room_context>` naming the same two members. Claim 4 is about
#      the turn a human started; this one is about a prompt built one spawn
#      later, from a task-local the spawn had to re-establish.
#
# ## The two identities are not optional
#
# `resolve_connect_auth` authorises a loopback peer on its first line, before it
# reads `bootstrap_ticket` — so a ticket redeemed over 127.0.0.1 binds no
# principal, silently and successfully. The members therefore connect over this
# host's LAN address, which is what `allow_insecure_remote` in the patched
# config is for. Everything runs under a scratch HOME/ALEPH_HOME: two processes
# on one vault is a documented way to lose vault data (PROCESS_MANAGEMENT.md).
#
# ## Why Node and not Python
#
# The gateway's only client transport is a WebSocket, the sibling fixtures reach
# it through Python's `websockets`, and on a Windows host there is no usable
# Python at all (the only `python3` on PATH is the WindowsApps stub) let alone
# that package, with no network to install one. Node ships a WHATWG WebSocket
# client and an HTTP server in core. A fixture that runs beats a fixture that
# matches its siblings' language.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18815}"
MOCK_PORT="${MOCK_PORT:-18915}"

command -v node >/dev/null 2>&1 || { echo "node is required for this fixture" >&2; exit 1; }

# --- build BEFORE the HOME redirect ----------------------------------------
# Deliberately ahead of `qa_redirect_home`, unlike the sibling fixtures. Their
# per-command `HOME="$REAL_HOME" cargo …` guard is correct on POSIX, where the
# pinned `RUSTUP_HOME`/`CARGO_HOME` are POSIX paths cargo understands. On
# Windows those pins are msys paths (`/c/Users/…`) that the native toolchain
# cannot read, so the only safe place for a cargo invocation here is before any
# of it happens. Nothing after this line runs cargo.
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  echo "=== build ==="
  (cd "$REPO" && cargo build --bin aleph-server 2>&1 | tail -3) || {
    echo "server build failed" >&2; exit 1; }
fi
TARGET_DIR="$(cd "$REPO" && cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | node -e 'let s="";process.stdin.on("data",c=>s+=c).on("end",()=>console.log(JSON.parse(s).target_directory))')"
SERVER="$TARGET_DIR/debug/aleph-server"
[ -x "$SERVER" ] || SERVER="$SERVER.exe"
[ -x "$SERVER" ] || { echo "no server binary under $TARGET_DIR/debug" >&2; exit 1; }

# --- scratch root ----------------------------------------------------------
# On Windows the root is kept in mixed form (`C:/…`) rather than the msys form
# (`/c/…`): bash accepts both, and the native `aleph-server` accepts only the
# first. A `/c/…` ALEPH_HOME resolves against the current drive root instead —
# silently, and into a tree the fixture would then fail to clean up.
if [ -z "${QA_ROOT:-}" ]; then
  QA_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-rooms-XXXXXX")"
  command -v cygpath >/dev/null 2>&1 && QA_ROOT="$(cygpath -m "$QA_ROOT")"
fi

. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
export REAL_HOME
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
WORKSPACE="$QA_ROOT/workspace"
REQUEST_LOG="$QA_ROOT/requests.jsonl"
DELETE_TARGET="$WORKSPACE/doomed.txt"
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

mkdir -p "$WORKSPACE"
printf 'QA workspace for the project-room tabs.\n' > "$WORKSPACE/README.md"
printf 'this file exists so an approved delete has an observable effect\n' > "$DELETE_TARGET"
: > "$REQUEST_LOG"

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
node "$HERE/patch_config.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" || exit 1

echo "=== start the mock provider ==="
node "$HERE/mock_llm.mjs" "$MOCK_PORT" "$REQUEST_LOG" "$DELETE_TARGET" >"$QA_ROOT/mock.log" 2>&1 &
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

echo "=== drive ==="
node "$HERE/drive_rooms.mjs" "$GATEWAY_PORT" "$WORKSPACE" "$REQUEST_LOG" "$DELETE_TARGET"
RC=$?

if [ "$RC" != "0" ]; then
  echo
  echo "--- server log tail ---"
  tail -60 "$QA_ROOT/server.log"
  echo "--- mock log tail ---"
  tail -40 "$QA_ROOT/mock.log"
fi

# HOLD=1 — keep the gateway up after the assertions so a BROWSER can be pointed
# at the state they just built.
#
# Why this mode has to exist: every assertion above is an RPC round-trip, so it
# proves the SERVER answers correctly and says nothing at all about whether the
# Panel's Kanban / Workspace / Memory components render that answer. Those three
# tabs were placeholders until this round, and "the RPC returns the team" and
# "the tab draws the team" are two different claims — the second one has no
# in-process test that can reach it (`aleph-panel --lib` renders components, not
# a live gateway).
#
# Loopback is always operator and credential-free, so a browser on this machine
# needs no ticket; that is also why the LAN-leg member checks above use a real
# credential and this does not.
#
# The values seeded above are deliberately ones no other machine could produce
# — "QA Room Renamed", "QA Room Team", "QA workspace" in README.md, the
# `qa-room-note` fact. They are the assertion AND the proof that the page being
# read is this fixture's, not some other server that happens to be listening.
if [ "${HOLD:-0}" = "1" ]; then
  echo
  echo "=== HOLD: gateway is still up ==="
  echo "  Panel:     http://127.0.0.1:$GATEWAY_PORT/"
  echo "  Room:      \"QA Room Renamed\" (renamed by the last phase)"
  echo "  Expect  -> Kanban:    a team card named \"QA Room Team\""
  echo "          -> Workspace: README.md, whose body contains \"QA workspace\""
  echo "          -> Memory:    a note titled \"QA Room Note\""
  echo "  scratch:   $QA_ROOT"
  echo "  Ctrl-C to tear down."
  # `wait` alone would return as soon as ANY child settles; this parks until the
  # server itself exits or the trap fires.
  while kill -0 "$SERVER_PID" 2>/dev/null; do sleep 2; done
fi

exit "$RC"
