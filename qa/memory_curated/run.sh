#!/usr/bin/env bash
# Real-machine QA for the memory tab: the curated hot tier's three verbs
# (`memory.curated.list|replace|remove`), the note window's grow-by-a-page
# control, and the partition contract every enumerating reader now resolves
# through (`gateway::handlers::memory_scope::read_partitions`) — the note list,
# the stat cards, the fix queue and the retrieval x-ray.
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
#     machine's loopback. Seen for real on 2026-08-21: the connected extension
#     said `isLocal: true` and still returned ERR_CONNECTION_REFUSED for both
#     127.0.0.1 and localhost while curl got 200. It was that extension
#     instance, not the Panel. Two lessons:
#       - Have a SECOND instrument. `chrome-devtools-mcp` drives a Chrome it
#         launches itself and does not go through the extension at all.
#       - `isLocal` is the browser describing itself. The cheap proof is data
#         that exists nowhere else: this fixture seeds ${NOTE_COUNT} notes into
#         a fresh scratch db, so the page reading "1040" IS the proof. Prefer
#         that over the self-report, and over a server log that may not even
#         record per-request lines (this one does not).
#   * `fill(uid, "")` sets `.value` without dispatching `input`; clear text
#     inputs with keystrokes. Same for scripted writes: go through the native
#     value setter and dispatch `input`, or Leptos never sees it.
#   * `textContent` is the RAW text; `innerText` is what the user sees. The
#     partition badge is uppercased by CSS, so it is `main__u-owner` in
#     textContent and `MAIN__U-OWNER` on screen. Matching the rendered string
#     against textContent finds nothing -- and "found nothing" reads exactly
#     like "the badge is not rendered", i.e. it would have been reported as a
#     product defect.
#   * A before/after comparison has to hold the READING FRAME constant. Diffing
#     the entry list across a rejected save reported "changed" because the card
#     was still in edit mode on the second read: the shape moved, not the
#     content. Leave the transient state first, then measure.
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
. "$HERE/../lib/build.sh"
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
  if ! qa_build --bin aleph-server; then
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

# The two ids are expected to DIFFER on a stock loopback install, and the seed
# has already asserted that the Panel's reader reaches the writer's rows anyway
# (`read_partitions` unions them). Printed rather than reconciled: an earlier
# revision re-keyed the corpus here to make the reader find it, which hid the
# defect and broke every retrieval surface in the process.

# The checklist heredoc below is UNQUOTED — it has to be, because it
# interpolates ${GATEWAY_PORT} and ${NOTE_COUNT}. That also means backticks and
# $(...) inside it RUN. Not hypothetical: `flag_user_correction` in item 10 was
# executed as a command, which printed "command not found" above the checklist
# and left the sentence with a hole exactly where the tool's name should be —
# i.e. the one word that item was telling you to care about.
#
# Escaping the instances is not the fix; the next one would land the same way.
# Fail here instead, by rule, before printing a checklist with a hole in it.
live="$(awk '/^cat <<CHECKLIST$/,/^CHECKLIST$/' "${BASH_SOURCE[0]}" \
  | sed 's/\\`//g; s/\\[$](/(/g' | grep -n -e '`' -e '[$](' || true)"
if [ -n "$live" ]; then
  echo "checklist heredoc contains live command substitution; escape it:" >&2
  echo "$live" >&2
  exit 70
fi

say "checklist"
cat <<CHECKLIST
Panel:  http://127.0.0.1:${GATEWAY_PORT}/memory  (左栏「列表」)
Probe:  python3 ${HERE}/probe.py ws://127.0.0.1:${GATEWAY_PORT}/ws ${ALEPH_HOME} <phase>
        phases: baseline | after-edit | after-remove | ledger | notes
                | fixes | xray | addressing
Agent the Panel asks about: ${PANEL_AGENT}
Partition the writers composed: ${WRITE_PARTITION}
  ^ these two DIFFER by design on a stock install. The readers resolve the base
    id into the union [org tier, this session's partition], which is why the
    Panel sees rows written under the second one.

--- A. curated hot tier (facet 「Hot Memory / 热区记忆」, first chip) ---
 1  The facet leads the bar and its badge reads 3 — the three entries the
    remember TOOL wrote, listed by the RPC face. A blank badge means the read
    has not run or failed; it is deliberately not a 0.
 2  Budget bar: used/limit and a percentage. Chars, not bytes — entry 3 is
    Chinese on purpose, so a byte count would over-report it by ~3x (21, not 57).
 3  Edit entry 2 -> Save. The list re-renders from the SERVER's snapshot (the
    reply carries the whole post-write state) and the toast says Saved.
      NOTE the toast auto-dismisses. Measured 2026-08-21: it appears +54ms
      after the click and is gone by +2478ms. One extra round trip does not fit
      in that window -- which is why an earlier round saw it once and then
      recorded a "timeout". So poll for it INSIDE the same evaluate() that
      clicks Save; "the toast was gone when I looked" is not evidence either
      way. The durable half of this claim is the probe below — the toast is a
      nicety, the file is the fact.
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

--- C. the third pillar's readers (D2: base id -> partition union) ---
10  Facet 「Feedback / 反馈」. The seeded correction is listed at the top as
    PENDING with severity medium:
      "QA-FIX stop reformatting the changelog when I only asked for a version bump"
      probe fixes  -> FIX_ROWS >= 1, and the seeded string is among them with
      status=pending. It was written by \`flag_user_correction\`, which composes
      the session scope — so a 0 here means the reader is back on the bare
      persona.
11  Console toolbar -> expand 「Retrieval / 检索透视」. Type a query that the
    seeded notes answer (e.g. "note-window growth check") and run it. The
    funnel lists stages with non-zero in/out counts and the result rows below
    are seeded notes.
      probe xray  -> XRAY_STAGES >= 1 and at least one stage with output > 0.
      A funnel of all 0 -> 0 is the D2 signature: every stage honestly reports
      nothing because it is probing a partition nothing wrote to.
12  Pick a note row and open it. The drawer loads a body (not "not found").
    This is the addressing half: the row reports its own partition and the
    drawer must use THAT, not the agent picker's id.
      probe addressing  -> the negative control that turns this from inference
      into proof. You cannot observe which id the drawer sent, so ask a verb
      where the two candidate ids give DIFFERENT answers: graph.node_detail is
      an ADDRESSING verb (verbatim partition, never a union), so the picker's
      bare id answers -32602 Note not found while the row's own partition
      answers with content. The drawer rendered content => it sent the row's.

Server log: ${QA_ROOT}/server.log
CHECKLIST

say "waiting (Ctrl-C to tear down)"
while kill -0 "$SERVER_PID" 2>/dev/null; do sleep 2; done
