#!/usr/bin/env bash
# Real-machine QA for the web search face (§3.18).
#
#   ./qa/web_search/run.sh reach     # a parameter the model named reaches the backend's query string
#   ./qa/web_search/run.sh order     # a backend that can carry the asked-for dimension is asked first
#   ./qa/web_search/run.sh degrade   # a dimension nobody can express is reported, not dropped in silence
#   ./qa/web_search/run.sh empty     # a zero-result answer does not end the chain
#   ./qa/web_search/run.sh fanout    # two named backends are both asked and their answers merge
#   ./qa/web_search/run.sh demote    # a backend that just failed is not asked again on the next search
#
#   KEEP=1 ./qa/web_search/run.sh reach     # keep the scratch dir for post-mortem
#
# ## Why this needs a booted server and a real turn
#
# In-process tests can assert that `SearchOptions::searxng_time_range` returns
# "week" and that `ordered_candidates` sorts a capable backend first. They
# cannot assert that a turn the model drives ends with `time_range=week` in an
# HTTP request, because that is a different object on a different path — and
# this repo has shipped four rounds of a feature whose only defect was the
# second one. Fifteen per-provider decoders existed for a freshness value no
# caller could set.
#
# ## What this fixture does NOT cover
#
# SearXNG is the only backend it can point at: seven of the nine hardcode
# their endpoint and firecrawl needs a credential. So these phases prove the
# WIRING — options reach a provider's request builder, the registry orders and
# fails over, the notes reach the model — and say nothing about the other
# eight backends' own request builders, which `capability_census.rs` covers at
# the source level instead. See README.md.
set -uo pipefail

PHASE="${1:-reach}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-websearch-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18821}"
MOCK_PORT="${MOCK_PORT:-18822}"
SEARX_A_PORT="${SEARX_A_PORT:-18823}"
SEARX_B_PORT="${SEARX_B_PORT:-18824}"

case "$PHASE" in
  reach|order|degrade|empty|fanout|demote) ;;
  *) echo "unknown phase: $PHASE (reach|order|degrade|empty|fanout|demote)" >&2; exit 64 ;;
esac

# Build BEFORE HOME is redirected: cargo's registry, git cache and rustup
# toolchain all live under the real HOME.
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
SEARX_A_LOG="$QA_ROOT/searxng-a.log"
SEARX_B_LOG="$QA_ROOT/searxng-b.log"

# A debug-built agent turn with tools overflows the 32 MB worker stack floor.
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

SERVER_PID=""
MOCK_PID=""
SEARX_A_PID=""
SEARX_B_PID=""
say() { printf '\n=== %s ===\n' "$*"; }
cleanup() {
  for pid in "$SERVER_PID" "$MOCK_PID" "$SEARX_A_PID" "$SEARX_B_PID"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null
  done
  sleep 1
  for pid in "$SERVER_PID" "$MOCK_PID" "$SEARX_A_PID" "$SEARX_B_PID"; do
    [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null
  done
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build ($PHASE)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  qa_build --bin aleph-server || { echo "build failed" >&2; exit 1; }
fi
# Ask cargo where its target dir really is: a shared absolute target-dir makes
# `$REPO/target` wrong from any git worktree.
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }
# The one trace a swallowed build failure leaves behind. See qa/lib/build.sh.
echo "binary: $BIN ($(date -r "$BIN" '+%Y-%m-%d %H:%M:%S'))"

say "generate a baseline config"
timeout 25 "$BIN" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
python3 "$BUSY/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" --max-pending-steering 8 || exit 1

case "$PHASE" in
  reach|degrade)
    python3 "$HERE/patch_search.py" "$CONFIG" \
      --searxng "searxng=$SEARX_A_PORT" --default searxng || exit 1
    ;;
  order)
    # searxng is the configured default; exa is a fallback. Exa is the only
    # backend in reach that declares `domain_filter`, so the ordering claim is
    # "the fallback outranks the default when the request needs what only the
    # fallback can carry". Exa then fails (its key is invalid), the chain
    # continues to searxng, and the answer's notes report the failure — which
    # is how a phase with no visibility into api.exa.ai can still see that it
    # was asked.
    python3 "$HERE/patch_search.py" "$CONFIG" \
      --searxng "searxng=$SEARX_A_PORT" --exa exa \
      --default searxng --fallback exa || exit 1
    ;;
  empty)
    python3 "$HERE/patch_search.py" "$CONFIG" \
      --searxng "empty=$SEARX_A_PORT" --searxng "full=$SEARX_B_PORT" \
      --default empty --fallback full || exit 1
    ;;
  demote)
    # A dead backend (503) as the default, a live one as its fallback. Both are
    # searxng, so they declare identical capabilities and the request asks for
    # no dimension — which leaves recent health as the only thing that can
    # reorder them. That is deliberate: a phase where capability could also
    # explain the order would not be measuring health.
    python3 "$HERE/patch_search.py" "$CONFIG" \
      --searxng "dead=$SEARX_A_PORT" --searxng "live=$SEARX_B_PORT" \
      --default dead --fallback live || exit 1
    ;;
  fanout)
    # Two backends, and deliberately NO fallback wiring between them: the
    # claim is that naming both on the tool face asks both, which must not be
    # confusable with the chain falling through from one to the other. With
    # `fallback_providers` empty, a chain run would only ever reach `alpha`.
    python3 "$HERE/patch_search.py" "$CONFIG" \
      --searxng "alpha=$SEARX_A_PORT" --searxng "bravo=$SEARX_B_PORT" \
      --default alpha || exit 1
    ;;
esac

say "write the turn's tool plan"
python3 - "$QA_ROOT/spec.json" "$PHASE" <<'PY'
import json, sys
out, phase = sys.argv[1], sys.argv[2]
# One call per arm. Attribution is by the query text, which `SearchOutput`
# echoes back — never by turn number: a run opens with a strategy-planner call
# that advances the mock's counter without emitting a tool call.
if phase == "reach":
    spec = {"name": "search", "input": {"query": "QA_ARM_REACH rust async", "recency": "week"}}
elif phase == "degrade":
    spec = {"name": "search",
            "input": {"query": "QA_ARM_DEGRADE rust", "domains": ["example.invalid"]}}
elif phase == "empty":
    spec = {"name": "search", "input": {"query": "QA_ARM_EMPTY rust"}}
elif phase == "demote":
    # Two identical plain searches. The second one is the measurement; the
    # first exists to make the backend fail once.
    spec = [
        {"name": "search", "input": {"query": "QA_ARM_DEMOTE_ONE rust"}},
        {"name": "search", "input": {"query": "QA_ARM_DEMOTE_TWO rust"}},
    ]
elif phase == "fanout":
    # Two arms. The first names both backends; the second names one, and is
    # the control — without it a green on "both logs have a request" could be
    # produced by anything that asks every configured backend.
    spec = [
        {"name": "search",
         "input": {"query": "QA_ARM_FANOUT rust", "providers": ["alpha", "bravo"]}},
        {"name": "search",
         "input": {"query": "QA_ARM_SOLO rust", "providers": ["alpha"]}},
    ]
else:
    spec = [
        {"name": "search",
         "input": {"query": "QA_ARM_DOMAINS rust", "domains": ["example.invalid"]}},
        {"name": "search", "input": {"query": "QA_ARM_PLAIN rust"}},
    ]
json.dump(spec, open(out, "w"))
PY

say "start mock backends"
SHARED_FLAG=""
[ "$PHASE" = "fanout" ] && SHARED_FLAG="--shared"
python3 "$HERE/mock_searxng.py" "$SEARX_A_PORT" "$SEARX_A_LOG" \
  $([ "$PHASE" = "empty" ] && echo --empty) \
  $([ "$PHASE" = "demote" ] && echo --fail) $SHARED_FLAG >"$QA_ROOT/searxng-a.out" 2>&1 &
SEARX_A_PID=$!
if [ "$PHASE" = "empty" ] || [ "$PHASE" = "fanout" ] || [ "$PHASE" = "demote" ]; then
  python3 "$HERE/mock_searxng.py" "$SEARX_B_PORT" "$SEARX_B_LOG" $SHARED_FLAG \
    >"$QA_ROOT/searxng-b.out" 2>&1 &
  SEARX_B_PID=$!
fi

say "start mock provider"
# `tool-chain`, not `quick`: a run opens with a strategy-planner call that
# carries no tool surface and still advances the mock's counter.
python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" /etc/hostname tool-chain \
  "$QA_ROOT/spec.json" "$QA_ROOT/requests.jsonl" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
sleep 1

say "start server"
"$BIN" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 90); do
  curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && break
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

say "drive: $PHASE"
RC=0
if [ "$PHASE" = "empty" ]; then
  QA_EXPECT_TAG="port$SEARX_B_PORT" python3 "$HERE/drive_search.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" "$PHASE" "$QA_ROOT/requests.jsonl" \
    "$SEARX_A_LOG" "$SEARX_B_LOG" || RC=$?
elif [ "$PHASE" = "fanout" ] || [ "$PHASE" = "demote" ]; then
  python3 "$HERE/drive_search.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" "$PHASE" "$QA_ROOT/requests.jsonl" \
    "$SEARX_A_LOG" "$SEARX_B_LOG" || RC=$?
else
  python3 "$HERE/drive_search.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" "$PHASE" "$QA_ROOT/requests.jsonl" \
    "$SEARX_A_LOG" || RC=$?
fi

say "backend request log"
echo "--- searxng A ---"; cat "$SEARX_A_LOG" 2>/dev/null | head -10
{ [ "$PHASE" = "empty" ] || [ "$PHASE" = "fanout" ] || [ "$PHASE" = "demote" ]; } && \
  { echo "--- searxng B ---"; cat "$SEARX_B_LOG" 2>/dev/null | head -10; }

exit "$RC"
