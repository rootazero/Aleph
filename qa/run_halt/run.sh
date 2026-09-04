#!/usr/bin/env bash
# Real-machine QA for what a run says about how it ended (§3.17c/d).
#
#   ./qa/run_halt/run.sh crash    # the provider refuses mid-run -> the FAILURE arm
#   ./qa/run_halt/run.sh cap      # max_iterations trips -> the SUCCESS arm + detail
#   ./qa/run_halt/run.sh receipt  # a capped run through real `aleph ask`, once per
#                                 # LC_ALL, against a real server
#   ./qa/run_halt/run.sh panel    # boot + hold, for the browser half (checklist)
#   KEEP=1 ./qa/run_halt/run.sh crash   # keep the scratch dir for post-mortem
#
# Exit code = number of failed claims.
#
# ## `crash` reported 4 failures for a round, and they were the product
#
# First run, 2026-08-29. A failed run emitted a terminal frame PER RETRY
# ATTEMPT:
#
#   run_complete{failed, loops:2, tokens:356}   <- honest, attempt 1
#   run_retrying                                 <- run_loop/inner.rs, attempt 2
#   run_complete{failed, loops:0, tokens:0}      <- the retried dispatch 401s at once
#   run_retrying
#   run_complete{failed, loops:0, tokens:0}
#   run_error
#
# Every client keeps the LAST terminal frame, so the receipt read 0 loops /
# 0 tools / 0 tokens — the §3.17c symptom, resurrected by a path that round did
# not consider.
#
# Two separate defects, both closed 2026-08-29, and this scenario now passes
# 6/6:
#
#   1. **The retry should never have happened.** `harness_bridge/error.rs`
#      classified 401/403 as transient while `llm_retry::is_permanent_failure`
#      — the predicate the circuit breaker acts on — called the identical error
#      permanent, and acted on it: `FailoverProvider` sheds a credential failure
#      on the first strike with a long cooldown. So the outer loop re-dispatched
#      into a chain whose breakers were already open, which is exactly why
#      attempts 2 and 3 came back instantly having done nothing. Two layers
#      answering "will this recover on its own?" in opposite directions; the one
#      that already had a consumer wins. The single auth failure that IS
#      recoverable — an expired OAuth token, because this process holds the
#      remedy — is checked first and still retries.
#   2. **A retried attempt must not speak.** Neither "first frame wins" nor
#      "last frame wins" fixes that at the client (first-wins reports a run that
#      retried and then SUCCEEDED as failed), so the frame has to be withheld
#      from an attempt that is about to be retried. The layer that classifies
#      the failure is not the layer that emits it — the drain has already
#      forwarded by the time the classification exists — so the drain now stops
#      one step short: it returns a prepared `HeldComplete` and
#      `run_dispatch_and_drain_classified` decides whether to send it, knowing
#      both the classification and this attempt's place in the retry budget.
#
# The `len(completes) == 1` assertion is kept exactly as it was written while it
# was red. A fixture edited to agree with the defect it found is a fixture that
# will agree with it forever.
#
# ## `receipt` could not pass either, for an unrelated reason, and that one was
# ## bigger than this fixture
#
# The CLI received NO stream frames from a real gateway. Measured side by side
# on the same socket at the same moment, 2026-08-29:
#
#   drive_halt.py (python websockets)  -> every frame, 5/5 claims pass
#   aleph watch                        -> its banner, and nothing else
#   aleph ask --json                   -> nothing at all, while the mock logged
#                                         the turns and the server ran the run
#
# Cause: the gateway published events as a bare
# `{"method":"stream.X","params":{…}}` while `shared/client` parsed them with
# `serde_json::from_str::<JsonRpcRequest>`, whose `jsonrpc: String` is a
# required field. Every frame failed with `missing field 'jsonrpc'` and was
# discarded behind one `debug!` line — and `shared/client` is the TUI's client
# too, so neither surface had ever received a `stream.*` frame from a real
# server. `aleph ask` parked forever on a run that had already finished.
#
# `gateway::events::frame_census` guards the other half of that same contract —
# that every `stream.*` method has a `StreamEvent` twin to decode into — and was
# green throughout: it checks the PAYLOAD, and the envelope carrying every
# payload was what dropped them.
#
# Both halves are fixed: the server builds the envelope with
# `JsonRpcRequest::notification` (one type, not two hand-written shapes) and the
# client reads `method`/`params`/`id` off a `Value` so a producer that forgets
# the version tag costs a warning rather than the event plane. `receipt` is what
# proves it end to end, and it is also the only thing that exercises
# `UiLocale::from_env` on a real machine — `render_summary_footer` resolves the
# language from the process environment, so "does LC_MESSAGES actually move it"
# is only answerable by running the binary twice.
#
# ## The instrument was broken too, and it is worth more than the finding
#
# The paragraph above was first measured against a `target/debug/aleph` that was
# sixteen days old. `cargo build --bin aleph` fails — that bin belongs to
# `aleph-cli`, and the invocation was missing `-p` — and the failure went
# through `| tail -5`, whose exit status is what `if !` tested. A failed build
# read as a successful one and the fixture ran whatever was already in the
# shared target dir. Every `run.sh` under `qa/` had that shape; they now share
# `qa/lib/build.sh::qa_build`, which checks `PIPESTATUS[0]`.
#
# The finding happened to be true anyway, which is the unnerving part: an
# instrument that agrees with reality by accident is still broken.
#
# ## Why this needs a real machine
#
# `terminate_reason` is written by the harness, adjusted by the orchestrator
# bridge, settled by the runner, forwarded-or-synthesized by the gateway drain,
# and rendered by five surfaces. §3.17c is what happens when every one of those
# has a passing test and the chain is still broken end to end: the terminal
# settle sat below `run_result.map_err(..)?`, so a run that spent 40 turns and
# 200k tokens before dying emitted `FlowOutcome::default()` — and that struct's
# `terminate_reason` is `Completed`. Every unit test on the producer side was
# green, because they all asserted the producer.
#
# The two arms are separate scenarios because they are separate code paths and
# the round fixed a different thing on each: `crash` takes the failure arm
# (the settle that was not there), `cap` takes the success arm (and is the only
# way to get `terminate_detail` populated, which is the field three of the five
# renderers ignored).
#
# ## The Panel half is manual, and that is the point
#
# `run_halts` is a live projection, keyed by run id and fed by the `run_complete`
# frame — like `run_costs` beside it, it is NOT rehydrated from `chat.history`.
# So a badge can only be seen by a browser that was already attached when the
# run ended, which is a thing a script cannot assert and a person can. `panel`
# boots the same rig and holds; the checklist it prints is the assertion.
#
# Same scratch-HOME discipline as every fixture here: build BEFORE $HOME is
# redirected (cargo's registry lives under the real one), then run with HOME
# *and* ALEPH_HOME inside a throwaway root.
set -uo pipefail

SCENARIO="${1:-crash}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
PLANH="$HERE/../plan_handoff"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-halt-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18801}"
MOCK_PORT="${MOCK_PORT:-18802}"
# The cap the `cap` scenario trips. Small so the scenario is quick; >1 so the
# run has real work in it before the cap, which is what makes "0 tokens" a
# provable lie rather than an accurate report of nothing.
MAX_ITERATIONS="${MAX_ITERATIONS:-3}"
# Tool-calling turns before the provider refuses, in `crash`/`receipt`.
BURN_TURNS="${BURN_TURNS:-2}"

case "$SCENARIO" in
  crash|cap|receipt|panel) ;;
  *) echo "unknown scenario: $SCENARIO (crash|cap|receipt|panel)" >&2; exit 64 ;;
esac

. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
OBS="$QA_ROOT/observations.jsonl"

# The 32 MB floor in main.rs::worker_stack_size is not enough for a debug-built
# agent run with tools.
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

MOCK_PID=""
SERVER_PID=""

say() { printf '\n=== %s ===\n' "$*"; }

cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build ($SCENARIO)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  # One invocation per package, and `-p` on both. `aleph` is a bin of
  # `aleph-cli`, not of the default-run package, so `cargo build --bin aleph`
  # fails with `no bin target named aleph` — which this fixture did not notice
  # for a whole round, because the failure went through a `| tail`. See
  # `qa/lib/build.sh` for what that cost.
  qa_build -p alephcore --bin aleph-server || exit 1
  if [ "$SCENARIO" = "receipt" ]; then
    qa_build -p aleph-cli --bin aleph || exit 1
  fi
fi
# `.cargo/config.toml` pins a shared absolute target dir, so `$REPO/target` is
# wrong from any git worktree — ask cargo.
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
CLI="$TARGET_DIR/debug/aleph"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

if [ "$SCENARIO" = "panel" ] && [ ! -f "$REPO/interfaces/webchat/dist/index.html" ]; then
  # A debug server reads the Panel's dist/ from disk (rust_embed debug mode);
  # an empty dist serves a blank page and every item "fails" for the wrong
  # reason.
  echo "interfaces/webchat/dist/ has no build — run \`just wasm\` first" >&2
  exit 69
fi

say "generate a baseline config"
# `--port` on the GENERATION boot. The config does not exist yet, so without
# it this boot binds the built-in default port — and if anything already holds
# that port (another fixture, a dev server, the operator's own daemon) the
# process exits before writing a config at all. The symptom is
# `no config generated at …`, which reads like a permissions or path problem;
# the cause is one line further up the log. Binding the port this run already
# owns makes the generation boot as isolated as the real one.
timeout 25 "$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
PATCH_ARGS=(--gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT"
            --max-pending-steering 8 --wake-fallback-secs 600)
# Through the shared patcher, not appended: a generated config already has an
# `[execution]` table and a second header of that name is `duplicate key` —
# the server then refuses to boot after printing a banner with the DEFAULT
# port, which reads like the wrong port rather than like a config error.
case "${PLAN:-$SCENARIO}" in cap|receipt) PATCH_ARGS+=(--max-iterations "$MAX_ITERATIONS") ;; esac
python3 "$BUSY/patch_config.py" "$CONFIG" "${PATCH_ARGS[@]}" || exit 1
# `bash` is not idempotent, so the default `auto` tier raises a confirmation
# card and the run would park on a human who is not there. An explicit `allow`
# outranks the tier (`effective_permission`) — the knob an operator would use,
# not a test-only bypass.
python3 "$PLANH/add_overrides.py" "$CONFIG" bash=allow || exit 1

# `receipt` rides the CAP plan, not the crash one, and the reason is an
# observation rather than a preference: against the crash plan `aleph ask`
# never returns. The run really does finish — the mock logs turns 1..5 and the
# server settles — and the CLI then sits there until `timeout` kills it, having
# printed nothing at all. That is worth knowing and it is not what this
# scenario is for; the cap plan reaches the same renderer down a path that
# terminates, and it additionally proves the detail-beats-reason precedence on
# the real binary (the umbrella token must NOT be what gets printed).
MOCK_PLAN="crash"
case "$SCENARIO" in cap|receipt) MOCK_PLAN="cap" ;; esac
# `panel` needs both plans and holds the server, so it cannot just tell the
# operator to "run the cap scenario next" — that one exits as soon as its
# claims are checked. `PLAN=cap ./qa/run_halt/run.sh panel` is how the second
# half of its checklist is actually reachable.
MOCK_PLAN="${PLAN:-$MOCK_PLAN}"
say "start mock provider (plan $MOCK_PLAN)"
python3 "$HERE/mock_halt.py" "$MOCK_PORT" "$MOCK_PLAN" "$OBS" "$BURN_TURNS" \
  >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
# Prove it bound before going any further. A mock that lost the port to a
# leftover from a previous run (`Address already in use`) leaves the scenario
# looking EXACTLY like a product failure: the server dials, gets nothing, and
# every claim fails for a reason that has nothing to do with the code. The
# instrument has to fail loudly and by name.
for _ in $(seq 1 20); do
  curl -sf -o /dev/null -m 1 -X POST -d '{}' "http://127.0.0.1:$MOCK_PORT/v1/messages" 2>/dev/null && break
  kill -0 "$MOCK_PID" 2>/dev/null || break
  sleep 0.5
done
if ! kill -0 "$MOCK_PID" 2>/dev/null; then
  echo "mock provider died on startup — is port $MOCK_PORT already taken?" >&2
  tail -5 "$QA_ROOT/mock.log" >&2
  exit 70
fi

say "start server"
# stdout is not a TTY here, so tracing goes to $ALEPH_HOME/logs/ — the redirect
# below catches only the startup banner. "No output" is not "nothing happened".
"$BIN" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 90); do
  curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && break
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

RC=0
case "$SCENARIO" in
  crash|cap)
    say "drive: $SCENARIO"
    python3 "$HERE/drive_halt.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$SCENARIO" || RC=$?
    ;;

  receipt)
    # The CLI face, end to end, against a real server — the half a unit test
    # cannot reach: `render_summary_footer` resolves the language from the
    # process environment, so "does LC_MESSAGES actually move it" is only
    # answerable by running the binary twice.
    [ -x "$CLI" ] || { echo "no CLI binary at $CLI" >&2; exit 1; }
    # `--server`, and it is now a real precedence rather than the only path:
    # the flag used to carry a clap `default_value`, so it was ALWAYS present
    # and `CliConfig.server` had no reader in this binary at all — a config file
    # pointing at a remote gateway was silently ignored while the CLI talked to
    # loopback. `--ca-cert` beside it had resolved flag ▸ file ▸ built-in since
    # it was written; `--server` now does too (same fix in `aleph-tui`, which
    # loads the same `CliConfig`). An explicit flag here is still the right call
    # for a fixture: it pins the port this run bound.
    SERVER_URL="ws://127.0.0.1:$GATEWAY_PORT/ws"
    # `</dev/null` is load-bearing, and it is an instrument caveat rather than a
    # product one. `ask::read_piped_stdin` reads stdin to EOF whenever stdin is
    # not a TTY — that is what makes `git diff | aleph ask "review"` work — and
    # a fixture inherits whatever pipe it was launched with. Without this the
    # CLI blocks before it sends anything: the mock logs zero requests, the
    # server logs nothing, and it reads exactly like a server that never
    # answered. `timeout` is the second half: a hang must fail the scenario, not
    # wedge it.
    ask() {
      env -u LANG -u LC_MESSAGES LC_ALL="$1" \
        timeout 90 "$CLI" --server "$SERVER_URL" ask "do some work" \
        </dev/null >"$2" 2>&1
    }
    say "receipt: LC_ALL=en_US.UTF-8"
    ask en_US.UTF-8 "$QA_ROOT/receipt.en"
    tail -6 "$QA_ROOT/receipt.en"
    say "receipt: LC_ALL=zh_CN.UTF-8"
    ask zh_CN.UTF-8 "$QA_ROOT/receipt.zh"
    tail -6 "$QA_ROOT/receipt.zh"

    say "claims"
    fail() { echo "[FAIL] $*"; RC=$((RC + 1)); }
    pass() { echo "[PASS] $*"; }
    # The rule for every line below: a NEGATIVE claim is only meaningful once
    # something positive has been shown about the same bytes. The first version
    # of this block asserted "the English receipt carries no Chinese" directly,
    # and it passed cheerfully against `Error: Connection refused` — a receipt
    # that was never printed carries no Chinese either. So the two positive
    # claims come first and the negatives are gated on them.
    EN_OK=0; ZH_OK=0
    grep -q "hit max iterations" "$QA_ROOT/receipt.en" \
      && { pass "the English receipt names the cap"; EN_OK=1; } \
      || fail "the English receipt has no 'hit max iterations': $(tail -3 "$QA_ROOT/receipt.en")"
    grep -q "已达迭代上限" "$QA_ROOT/receipt.zh" \
      && { pass "the Chinese receipt names the cap"; ZH_OK=1; } \
      || fail "the Chinese receipt has no '已达迭代上限': $(tail -3 "$QA_ROOT/receipt.zh")"

    if [ "$EN_OK$ZH_OK" = "11" ]; then
      grep -q "已达迭代上限" "$QA_ROOT/receipt.en" \
        && fail "the English receipt leaked Chinese — LC_ALL did not move it" \
        || pass "LC_ALL really selects the language (en carries no Chinese label)"
      # The half `aleph exec` used to get wrong: `terminate_reason` here is the
      # umbrella `budget_exhausted_partial_result`, and only `terminate_detail`
      # says which budget was hit. A receipt printing the umbrella is a receipt
      # that never read the field.
      grep -qE "partial result|部分结果|保留了部分" "$QA_ROOT/receipt.en" "$QA_ROOT/receipt.zh" \
        && fail "the receipt printed the umbrella instead of the cap — terminate_detail unread" \
        || pass "the receipt names the cap, not the budget umbrella"
      # And the fall-through that used to swallow every unknown token.
      grep -qE "已结束" "$QA_ROOT/receipt.en" "$QA_ROOT/receipt.zh" \
        && fail "the neutral fall-through label is back" \
        || pass "no run is badged with the neutral 'ended' fall-through"
    else
      fail "skipped the three negative claims: a receipt that was never printed \
satisfies every negative assertion, so they would all have passed for free"
    fi
    ;;

  panel)
    cat <<CHECKLIST

=== the Panel half — drive this in a browser, then Ctrl-C ===

  Panel:  http://127.0.0.1:$GATEWAY_PORT/

  The badge is a LIVE projection (\`ChatState::run_halts\`, fed by the
  \`run_complete\` frame). It is not rehydrated from \`chat.history\`, so it can
  only be seen by a browser that was already attached when the run ended.
  Reloading after the fact and finding nothing is the design, not a bug.

  1. Open the Panel and send any message. The mock burns $BURN_TURNS tool-calling
     turns and then refuses with HTTP 401.
  2. The assistant bubble must grow a SECOND meta line under the cost line:
     a ⚠️ glyph plus a halt label. In English: "failed". In Chinese: "运行失败".
     (Settings -> General switches the Panel's language; it is a browser-side
     setting and deliberately independent of the terminal's LC_MESSAGES.)
  3. Hover the badge: the \`title\` must read \`terminate_reason: failed\`.
  4. The cost line beside it must show non-zero tokens. A failed run used to
     report 0 — that is the §3.17c regression, and it is visible right here.
  5. Send a second message. The new bubble must NOT inherit the first badge.

  Then boot \`PLAN=cap ./qa/run_halt/run.sh panel\` and repeat 1-3: the label
  must name the cap ("hit max iterations" / "已达迭代上限") and NOT the umbrella
  ("budget exhausted (partial result)" / "预算耗尽（保留了部分结果）").

CHECKLIST
    echo "holding. Ctrl-C to tear down."
    while kill -0 "$SERVER_PID" 2>/dev/null; do sleep 5; done
    ;;
esac

say "mock provider log"
tail -20 "$QA_ROOT/mock.log"

say "halt-related server log lines"
LOGDIR="$ALEPH_HOME/logs"
if [ -d "$LOGDIR" ]; then
  grep -iE 'terminate|run_complete|HarnessError|authentication' "$LOGDIR"/aleph-server.log* 2>/dev/null \
    | tail -20 || echo "(no halt lines)"
else
  tail -20 "$QA_ROOT/server.log"
fi

say "verdict: rc=$RC"
exit "$RC"
