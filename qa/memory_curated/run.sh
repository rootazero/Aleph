#!/usr/bin/env bash
# Real-machine QA for the memory tab's round-2 surfaces: the curated hot tier's
# three verbs (`memory.curated.list|replace|remove`) and the note window's
# grow-by-a-page control.
#
#   ./qa/memory_curated/run.sh          # boot an isolated server, seed, print the checklist
#   KEEP=1 ./qa/memory_curated/run.sh   # keep the scratch dir for post-mortem
#
# BOOTS, SEEDS AND WAITS. The browser half is driven by the agent against the
# checklist this prints; `probe.py` answers the out-of-band questions (what is
# on disk, what the *tool* face sees) at each checkpoint.
#
# ## Why the seed goes through `tools.invoke` and not the database
#
# Both claims are about two faces of ONE store agreeing, so a fixture that
# writes the store itself would be asserting against its own idea of the
# layout. `remember` and `note_manage` are the production writers, reached
# through the gateway exactly as an agent turn reaches them — no model needed,
# because neither tool calls one. The curated file's path in particular is
# resolved by `MemoryContextProvider::get_or_load_curated_store` from the
# ambient scope, and hand-computing it here would be a second answer to the
# question the whole round is about.
#
# ## One provider, pointed at a closed port
#
# Not zero: `tools.invoke` dispatches through the builtin tool registry, and
# that registry only exists on the real-execution branch of
# `register_agent_handlers`, which is selected by "is an API key available".
# With no provider the seed dies on every call with
# `tools.invoke requires ToolRegistry (boot phase 2)` — a tool that needs no
# model still needs the face to exist. The configured provider dials a port
# nothing listens on, so it cannot complete a turn either; the daemons that
# WOULD write memory in the background are disabled explicitly rather than
# left starved (see patch_config.py).
#
# ## Instrument caveats inherited from earlier rounds (qa/picker_nav/run.sh)
#
#   * A browser that reports itself as local may still fail to reach this
#     machine's loopback. Check the server log for the request before believing
#     the Panel is broken.
#   * `fill(uid, "")` sets `.value` without dispatching `input`; clear text
#     inputs with keystrokes.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-memcur-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18811}"
# Nothing listens here; see the provider note above.
DEAD_PORT="${DEAD_PORT:-18812}"
# Above NOTE_WINDOW (1000) by one short page: the control only exists when the
# window is truncated, and a second page of 40 makes "page 21 did not exist
# before the click" a visible, countable thing.
NOTE_COUNT="${NOTE_COUNT:-1040}"

. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --no-deps --format-version 1 2>/dev/null \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])' 2>/dev/null)"
BIN="${TARGET_DIR:-$REPO/target}/debug/aleph-server"
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
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }
# Debug servers read the Panel's dist/ from disk (rust_embed debug mode); an
# empty dist serves a blank page and every item "fails" for the wrong reason.
if [ ! -f "$REPO/interfaces/webchat/dist/index.html" ]; then
  echo "interfaces/webchat/dist/ has no build — run \`just wasm\` first" >&2
  exit 69
fi

say "generate a baseline config"
timeout 25 "$BIN" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config (inert daemon, one unreachable provider)"
python3 "$HERE/patch_config.py" "$CONFIG" --gateway-port "$GATEWAY_PORT" \
  --dead-port "$DEAD_PORT" || exit 1

say "boot"
"$BIN" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 80); do
  curl -sf "http://127.0.0.1:$GATEWAY_PORT/health" >/dev/null 2>&1 && break
  sleep 0.5
done
if ! curl -sf "http://127.0.0.1:$GATEWAY_PORT/health" >/dev/null 2>&1; then
  echo "server did not come up"; tail -40 "$QA_ROOT/server.log"; exit 1
fi

say "seed (real writers, over the wire)"
python3 "$HERE/seed.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$ALEPH_HOME" "$NOTE_COUNT" || exit 1


PANEL_AGENT="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["panel_agent"])' "$ALEPH_HOME/qa-seeded.json")"
WRITE_PARTITION="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["write_partition"])' "$ALEPH_HOME/qa-seeded.json")"

say "relocate the note corpus into the partition the Panel reads"
# NOT a convenience — see relocate_notes.py's docstring. The Panel's note
# readers do not compose the session scope, so the corpus the real writer just
# produced is invisible to them. Skipped when the two ids already agree, which
# is what it would look like once that is fixed.
if [ "$PANEL_AGENT" != "$WRITE_PARTITION" ]; then
  python3 "$HERE/relocate_notes.py" "$ALEPH_HOME" \
    --from-agent "$WRITE_PARTITION" --to-agent "$PANEL_AGENT" || exit 1
else
  echo "panel agent == write partition ($PANEL_AGENT); nothing to relocate"
fi

say "checklist"
cat <<CHECKLIST
Panel:  http://127.0.0.1:${GATEWAY_PORT}/memory  (左栏「列表」)
Probe:  python3 ${HERE}/probe.py ws://127.0.0.1:${GATEWAY_PORT}/ws ${ALEPH_HOME} <phase>
        phases: baseline | after-edit | after-remove | ledger | notes
Agent the Panel asks about: ${PANEL_AGENT}
Partition the writers composed: ${WRITE_PARTITION}

--- A. curated hot tier (facet 「Hot Memory / 热区记忆」, first chip) ---
 1  The facet leads the bar and its badge reads 3 — the three entries the
    remember TOOL wrote, listed by the RPC face. A blank badge means the read
    has not run or failed; it is deliberately not a 0.
 2  Budget bar: used/limit and a percentage. Chars, not bytes — entry 3 is
    Chinese on purpose, so a byte count would over-report it by ~3x (21, not 57).
 3  Edit entry 2 -> Save. The list re-renders from the SERVER's snapshot (the
    reply carries the whole post-write state) and the toast says Saved.
      probe after-edit --new T --old T
      -> MEMORY.md on disk holds the new text and not the old, and
         remember{add: <new text>} is refused as a DUPLICATE, i.e. the tool's
         store is the same store the Panel just wrote.
 4  Remove entry 3. Badge 2, budget drops.
      probe after-remove --gone T  -> gone from the file, and the tool accepts
      a re-add (it is absent from the tool's view too, not just from the file).
 5  Expand 「Write attempts / 写入尝试」. Rows exist, newest first, each with a
    server-side reason rendered verbatim — including the seeded duplicate.
      probe ledger  -> LEDGER_ROWS=N must match what the Panel shows.
 6  Edit entry 1 to something over budget (paste >2200 chars). The server
    refuses, the toast carries its message, and THE LIST DOES NOT CHANGE.
      probe ledger  -> still N. Panel-side curation is deliberately not a
      model write attempt, so it must not appear in this ledger.

--- B. note window growth (facet 「All Notes / 全部笔记」) ---
 7  Badge reads 1000 while the store holds ${NOTE_COUNT}. Under the list:
    「Loaded 1000 of ${NOTE_COUNT}」 + 「Load more」. Pager: 20 pages.
 8  Click Load more. The button reads Loading… and then the whole line
    disappears (nothing left to load). Badge ${NOTE_COUNT}. Pager: 21 pages.
 9  Go to the last page. It renders the tail — rows page 21 could not have
    shown before, because page 21 did not exist.

Server log: ${QA_ROOT}/server.log
CHECKLIST

say "waiting (Ctrl-C to tear down)"
while kill -0 "$SERVER_PID" 2>/dev/null; do sleep 2; done
