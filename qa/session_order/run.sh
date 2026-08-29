#!/usr/bin/env bash
# Real-machine QA for the transcript's order and for `session.truncate`.
#
#   ./qa/session_order/run.sh          # both backends, then diff them
#   KEEP=1 ./qa/session_order/run.sh   # keep the scratch dir for post-mortem
#
# The round settled that the transcript's order is the order its rows were
# recorded — `messages.id` on SQLite, file position in the file store — and
# that the stamp is a predicate (`before`), never a rank. Unit tests build both
# stores in one process and assert they agree; they are blind to the config key
# that picks one, and they never touch the RPC face where `session.truncate`
# lives. Both gaps have already produced a shipped defect:
#
#   * `default_session_store_backend()` returned "file" while its own doc said
#     `"sqlite" (default)`.
#   * `session.truncate` answered INTERNAL_ERROR to every call on the SQLite
#     backend — two `unchecked_transaction()`s, the first shadowed rather than
#     committed — so `/undo` had never once succeeded there.
#
# Each backend is driven in its own scratch ALEPH_HOME, so neither can inherit
# the other's state and "they agree" cannot mean "they read the same file".
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-order-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18797}"
MOCK_PORT="${MOCK_PORT:-18998}"
TURNS="${TURNS:-4}"
KEEP_COUNT="${KEEP_COUNT:-4}"

# Build BEFORE HOME is redirected: cargo's registry, git cache and rustup
# toolchain all live under the real HOME, and a build launched with the scratch
# one silently degrades into a full network fetch that then times out.
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

MOCK_PID=""
SERVER_PID=""

say() { printf '\n=== %s ===\n' "$*"; }

stop_server() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  for _ in $(seq 1 40); do
    kill -0 "$SERVER_PID" 2>/dev/null || break
    sleep 0.25
  done
  kill -9 "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
}

cleanup() {
  stop_server
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! qa_build --bin aleph-server; then
    echo "build failed" >&2; exit 1
  fi
fi
# Ask cargo where its target dir really is: `.cargo/config.toml` pins a shared
# absolute one, so a hardcoded `$REPO/target` is wrong from any git worktree.
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

# A mock PER BACKEND, on its own port. The turn counter is process-global and
# the reply text is `mock turn N`, so one shared mock would hand the second
# backend turns 5..8 while the first got 1..4 — and the cross-backend
# comparison would then fail on a difference this fixture invented.
start_mock() {
  local port="$1" tag="$2"
  python3 "$BUSY/mock_anthropic.py" "$port" /etc/hostname single-shot \
    >"$QA_ROOT/$tag-mock.log" 2>&1 &
  MOCK_PID=$!
  sleep 1
}

stop_mock() {
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  MOCK_PID=""
}

wait_for_gateway() {
  for _ in $(seq 1 90); do
    curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && return 0
    kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$1"; return 1; }
    sleep 1
  done
  echo "gateway never came up"; tail -40 "$1"; return 1
}

# One backend, end to end: drive -> capture -> stop -> scramble -> restart ->
# capture -> truncate -> capture.
drive_backend() {
  local backend="$1"
  local mock_port="$2"
  local home="$QA_ROOT/$backend"
  export ALEPH_HOME="$home"
  mkdir -p "$ALEPH_HOME"
  local config="$ALEPH_HOME/config.toml"
  local db="$ALEPH_HOME/data/sessions.db"

  start_mock "$mock_port" "$backend"

  say "[$backend] generate a baseline config"
  timeout 25 "$BIN" start >"$QA_ROOT/$backend-gen.log" 2>&1 &
  local gen=$!
  for _ in $(seq 1 50); do [ -f "$config" ] && break; sleep 0.5; done
  kill "$gen" 2>/dev/null; wait "$gen" 2>/dev/null
  [ -f "$config" ] || { echo "no config at $config"; tail -20 "$QA_ROOT/$backend-gen.log"; return 1; }

  say "[$backend] patch config"
  python3 "$BUSY/patch_config.py" "$config" \
    --gateway-port "$GATEWAY_PORT" --mock-port "$mock_port" \
    --max-pending-steering 8 --wake-fallback-secs 600 || return 1
  python3 "$HERE/patch_backend.py" "$config" "$backend" || return 1

  say "[$backend] start server, drive $TURNS turns"
  "$BIN" start >"$QA_ROOT/$backend-server.log" 2>&1 &
  SERVER_PID=$!
  wait_for_gateway "$QA_ROOT/$backend-server.log" || return 1
  python3 "$HERE/drive_session_order.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" "$db" "$QA_ROOT/$backend-before.json" before \
    --turns "$TURNS" || return 1

  # The store the server booted must be the one the config named. `.aleph`
  # keeps a `sessions.db` either way (`session_events` is always SQLite), so
  # "the db exists" proves nothing; the transcript directory does.
  local key
  key="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1]))["session_key"])' \
        "$QA_ROOT/$backend-before.json")"
  say "[$backend] session $key"
  if [ "$backend" = "file" ]; then
    [ -d "$ALEPH_HOME/data/sessions" ] || { echo "file backend wrote no transcript dir"; return 1; }
  else
    if [ -d "$ALEPH_HOME/data/sessions" ] && \
       [ -n "$(find "$ALEPH_HOME/data/sessions" -name transcript.jsonl 2>/dev/null)" ]; then
      echo "sqlite backend wrote a transcript.jsonl — the config key did not take"; return 1
    fi
  fi

  say "[$backend] stop server, scramble the stamps"
  stop_server
  python3 "$HERE/scramble_stamps.py" "$ALEPH_HOME" "$backend" "$key" || return 1

  say "[$backend] restart, re-read, truncate to $KEEP_COUNT"
  "$BIN" start >>"$QA_ROOT/$backend-server.log" 2>&1 &
  SERVER_PID=$!
  wait_for_gateway "$QA_ROOT/$backend-server.log" || return 1
  python3 "$HERE/drive_session_order.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" "$db" "$QA_ROOT/$backend-after.json" after \
    --session-key "$key" --keep "$KEEP_COUNT" || return 1
  stop_server
  stop_mock
}

RC=0
port="$MOCK_PORT"
for backend in file sqlite; do
  drive_backend "$backend" "$port" || { echo "[$backend] scenario failed"; RC=1; stop_mock; }
  port=$((port + 1))
done

if [ "$RC" != "0" ]; then
  say "verdict: a backend never completed; nothing to compare"
  exit "$RC"
fi

say "compare"
python3 "$HERE/compare_backends.py" \
  "$QA_ROOT/file-before.json" "$QA_ROOT/file-after.json" \
  "$QA_ROOT/sqlite-before.json" "$QA_ROOT/sqlite-after.json" \
  "$TURNS" "$KEEP_COUNT"
RC=$?

say "verdict: rc=$RC"
exit "$RC"
