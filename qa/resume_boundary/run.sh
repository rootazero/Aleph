#!/usr/bin/env bash
# Real-machine QA for the crash boundary's TEXT — the sentence a dangling tool
# call is answered with, and whether it actually reaches the model.
#
#   ./qa/resume_boundary/run.sh crash      # a dangling call gets OUTCOME UNKNOWN,
#                                          # and the model's NEXT request carries it
#   ./qa/resume_boundary/run.sh attribute  # a dangle left by an EARLIER run is not
#                                          # blamed on this restart
#   KEEP=1 ./qa/resume_boundary/run.sh crash
#
# Why a real machine. `resume_coordinator.rs`'s unit tests and
# `tests/resume_coordinator_integration.rs` both assert on the bytes
# `boundary_repair_text` returns and on the event the coordinator appends —
# i.e. they test the PRODUCER. Neither shows those bytes ever entering a
# prompt: throw away everything downstream of the event append and both
# suites still pass. The oracle here is the mock provider's REQUEST LOG —
# what was actually put in front of the model on the next turn — not the
# server's event log.
#
# `attribute` is the falsifying arm for the defect this round's design spec
# (§1.4) fixes: run it on the pre-round tree and it must FAIL, both dangles
# misattributed to "the server restarted" instead of the older one reading
# "an earlier run in this session".
#
# How the dangle is made to happen: the mock's first turn answers with a
# `bash` tool_use of `sleep 120` — a call that will not get a result on any
# timescale this fixture runs on — and the driver kills the server with
# `kill -9` (not SIGTERM: a clean shutdown lets in-flight work settle and
# there would be nothing left to repair) a few hundred ms after the durable
# event log records the dispatch. `drive_dangle.py --mode send` does not
# guess that timing with a sleep; it polls the event log itself for the new
# `tool_call_requested` row before returning, so the kill lands exactly once
# the call is durably dangling and no earlier.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
PLANH="$HERE/../plan_handoff"
STAGE="${1:-crash}"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-resume-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18831}"
MOCK_PORT="${MOCK_PORT:-18832}"

case "$STAGE" in
  crash|attribute) ;;
  claims|denied|rewind|knobs|holes) ;;
  *) echo "unknown stage: $STAGE (crash|attribute|claims|denied|rewind|knobs|holes)" >&2; exit 64 ;;
esac

# Build BEFORE HOME is redirected: cargo's registry/git-cache/toolchain all
# live under the real HOME.
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
# The durable event log: a single sqlite file, opened at
# `SessionManagerConfig::default().db_path` (`src/gateway/session_manager/mod.rs`),
# which resolves through `get_sessions_db_path()` (`src/utils/paths.rs`) to
# `<ALEPH_HOME>/data/sessions.db`. `qa/busy_input/lib.py::SessionLog` already
# reads this exact path in every other fixture here.
EVENTS_DB="$ALEPH_HOME/data/sessions.db"
REQUEST_LOG="$QA_ROOT/request_log.jsonl"
SESSION_FILE="$QA_ROOT/session_key.txt"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

MOCK_PID=""
SERVER_PID=""
say() { printf '\n=== %s ===\n' "$*"; }

# Poll a file for a substring rather than guessing a sleep. The repair text
# is written to $REQUEST_LOG the instant the mock receives the resumed run's
# request — well before the mock answers it — so this only has to wait for
# boot + `wait_for_channel_config_snapshot` + one LLM round trip.
wait_for_text() {
  local file="$1" text="$2" budget="${3:-120}"
  local end=$((SECONDS + budget))
  while [ "$SECONDS" -lt "$end" ]; do
    if [ -f "$file" ] && grep -qF "$text" "$file" 2>/dev/null; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}

start_server() {
  "$BIN" start >>"$QA_ROOT/server.log" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 90); do
    curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && return 0
    kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died on boot" >&2; tail -40 "$QA_ROOT/server.log" >&2; return 1; }
    sleep 0.5
  done
  echo "server did not come up" >&2
  return 1
}

# kill -9, not SIGTERM: a clean shutdown closes the dangling call and there
# would be nothing left to repair — the fixture would be measuring nothing.
hard_kill_server() {
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$SERVER_PID" ] && wait "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
}

cleanup() {
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  qa_build -p alephcore --bin aleph-server || { echo "build failed" >&2; exit 1; }
fi
# `.cargo/config.toml` pins a shared absolute target dir, so `$REPO/target` is
# wrong from any git worktree — ask cargo.
# Ask cargo, then parse with whichever of node/python3 this host actually has.
# A `python3` that is the Windows `WindowsApps` stub prints NOTHING, so the
# command substitution below yields an empty path and the fixture goes looking
# for a binary at `/debug/aleph-server` — a message that reads like a build
# failure and is not one. (What the stub does with its exit code was written
# here as "exits 0" for two rounds and was never measured; measured on this
# host 2026-09-03, both `python3` and `python` exit **49**. The symptom is the
# same either way because it is the captured OUTPUT that is empty — but the
# difference matters downstream, where a non-zero exit is something a stage can
# refuse on. See the round-1 guard below.)
META="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null)"
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
  # An operator (or this repo's own build recipe) who pinned a shared target
  # dir built the binary THERE; `cargo metadata` answers with the workspace's
  # default and would send the fixture to an empty directory.
  TARGET_DIR="$CARGO_TARGET_DIR"
elif command -v node >/dev/null 2>&1; then
  TARGET_DIR="$(printf '%s' "$META" | node -e 'let s="";process.stdin.on("data",c=>s+=c).on("end",()=>console.log(JSON.parse(s).target_directory))')"
else
  TARGET_DIR="$(printf '%s' "$META" | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
fi
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || BIN="$BIN.exe"
[ -x "$BIN" ] || { echo "no binary at $TARGET_DIR/debug/aleph-server" >&2; exit 1; }
# `cargo clippy --all-targets` replaces every linked binary in this directory
# with an EMPTY file (clippy-driver never links, but still writes the artifact).
# The file is still executable, so `-x` above is satisfied and the run limps on
# to "no config generated" — a message that reads like a server bug. Measured
# on this host 2026-09-03, right after `cargo clippy --workspace --all-targets`.
[ -s "$BIN" ] || {
  echo "$BIN is 0 bytes — a clippy --all-targets run emptied it; rebuild with SKIP_BUILD=0" >&2
  exit 1
}

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
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG" >&2; tail -20 "$QA_ROOT/gen.log" >&2; exit 1; }

# ---------------------------------------------------------------------------
# Round-2 stages. Node, not Python: this host has no usable `python3` at all —
# both `python3` and `python` resolve to the Windows `WindowsApps` stub, which
# prints nothing and exits 49 (measured 2026-09-03; an earlier version of this
# comment said "exits 0 having done nothing", which was inherited rather than
# measured). The round-1 `crash` / `attribute` stages below stay Python — they
# were measured on a host that had one, and rewriting a green fixture to prove
# nothing new is churn; what they get instead is a guard that says so.
# ---------------------------------------------------------------------------
if [ "$STAGE" != "crash" ] && [ "$STAGE" != "attribute" ]; then
  command -v node >/dev/null 2>&1 || { echo "node is required for the r2 stages" >&2; exit 1; }
  # The native server and node both read Windows paths; the msys form reaches
  # neither.
  QA_ROOT_M="$QA_ROOT"
  command -v cygpath >/dev/null 2>&1 && QA_ROOT_M="$(cygpath -m "$QA_ROOT")"
  R2_REQUESTS="$QA_ROOT/requests.jsonl"
  RECEIPT="$QA_ROOT/receipt.json"
  : > "$R2_REQUESTS"
  # Every `drive` invocation is its own node process with its own counters, so
  # the last line a stage prints is whichever phase ran last — for `claims`
  # that is `cost`, which asserts nothing and prints `0 passed, 0 failed`.
  # Tailing a green stage therefore reads as "this measured nothing", and the
  # expensive half of that is the converse: a phase whose assertions all
  # vanished — an early `return`, a renamed wire key that makes the driver bail
  # before its checks, a `case` arm that stops being reached — prints the SAME
  # line and still exits 0. So the per-phase counts are summed here and
  # compared against a floor below (判据 #2: a stage that asserts nothing is
  # not green, it is unmeasured; the four faces of a predicate that never goes
  # red include "not installed").
  ASSERTS=0
  drive() {
    local out rc n
    out="$(node "$HERE/drive_r2.mjs" "$GATEWAY_PORT" "$QA_ROOT_M" "$@" 2>&1)"
    rc=$?
    printf '%s\n' "$out"
    n="$(printf '%s\n' "$out" | sed -n 's/^\([0-9][0-9]*\) passed, [0-9][0-9]* failed$/\1/p' | tail -1)"
    [ -n "$n" ] && ASSERTS=$((ASSERTS + n))
    return "$rc"
  }
  # How the dangle is MADE on this host — patch_r2.mjs's header, point 3, has
  # the two measurements that rule out a long-running command. `ask` parks the
  # dispatched call on a card nobody answers, which IS "dispatched, no
  # receipt"; `allow` is what the burst stage wants (many fast event pairs).
  # `deny` is NOT the `denied` stage's instrument — a statically denied call is
  # answered in the same turn, so it is never dangling; see
  # `drive_r2.mjs::cmdForgeDenial`.
  BASH_POLICY="ask"
  [ "$STAGE" = "holes" ] && BASH_POLICY="allow"
  BURST="${QA_BURST:-40}"

  say "patch config (node)"
  node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" || exit 1

  say "start mock provider (node)"
  QA_BURST="$BURST" node "$HERE/mock_r2.mjs" "$MOCK_PORT" "$R2_REQUESTS" >"$QA_ROOT/mock.log" 2>&1 &
  MOCK_PID=$!
  for _ in $(seq 1 40); do
    curl -sf -o /dev/null -m 1 "http://127.0.0.1:$MOCK_PORT/v1/models" 2>/dev/null && break
    kill -0 "$MOCK_PID" 2>/dev/null || break
    sleep 0.25
  done
  kill -0 "$MOCK_PID" 2>/dev/null || { echo "mock died on startup — port $MOCK_PORT taken?" >&2; tail -5 "$QA_ROOT/mock.log" >&2; exit 70; }

  RC=0
  # The floor each stage's phases have to reach to be called green. These are
  # MEASURED values, not targets: each is the count the stage printed on the
  # tree that introduced it (2026-09-03, this worktree). Adding an assertion
  # raises the number here in the same commit; a number that drops on its own
  # is the defect this guard exists for.
  case "$STAGE" in
    claims) FLOOR=13 ;;
    denied) FLOOR=5 ;;
    rewind) FLOOR=11 ;;
    knobs)  FLOOR=10 ;;
    holes)  FLOOR=12 ;;
    *)      FLOOR=0 ;;
  esac
  case "$STAGE" in
    claims)
      # Boot with resume OFF so the receipt below is the ONLY pass over this
      # log: a boot scan that already repaired it would make every counter on
      # the receipt read zero and the wire face read `clean`, and both would
      # look like the feature working.
      node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" false "$BASH_POLICY" >/dev/null || exit 1
      start_server || exit 1
      drive dangle qa-dangle || { echo "instrument failure: no dangle" >&2; RC=1; }
      hard_kill_server
      [ "$RC" = "0" ] && { drive assert-dangling 1 || RC=1; }
      [ "$RC" = "0" ] && { start_server || exit 1; }
      [ "$RC" = "0" ] && { drive claims-wire || RC=1; }
      if [ "$RC" = "0" ]; then
        say "aleph-server resume --json"
        "$BIN" resume --json "$(cat "$SESSION_FILE")" >"$RECEIPT" 2>"$QA_ROOT/resume.err"
        echo "resume rc=$? receipt:"; cat "$RECEIPT"; tail -5 "$QA_ROOT/resume.err"
        drive claims-receipt "$RECEIPT" || RC=1
        drive cost || RC=1
      fi
      ;;
    denied)
      # Resume OFF for the same reason as `claims`: the receipt below must be
      # the only pass over this log. The denial itself is appended between the
      # kill and the restart, with the server down — the one row a crash inside
      # the denial window would have left, and the only half of this shape a
      # fixture outside the process can produce (drive_r2.mjs::cmdForgeDenial
      # carries the two measurements).
      node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" false "$BASH_POLICY" >/dev/null || exit 1
      start_server || exit 1
      drive dangle qa-dangle || { echo "instrument failure: no dangle" >&2; RC=1; }
      hard_kill_server
      [ "$RC" = "0" ] && { drive assert-dangling 1 || RC=1; }
      [ "$RC" = "0" ] && { drive forge-denial || RC=1; }
      [ "$RC" = "0" ] && { start_server || exit 1; }
      [ "$RC" = "0" ] && { drive denied wire || RC=1; }
      if [ "$RC" = "0" ]; then
        say "aleph-server resume --json"
        "$BIN" resume --json "$(cat "$SESSION_FILE")" >"$RECEIPT" 2>"$QA_ROOT/resume.err"
        echo "resume rc=$? receipt:"; cat "$RECEIPT"
        drive denied model || RC=1
      fi
      ;;
    rewind)
      # Resume OFF: a boot scan that repaired and re-ran the session would
      # leave nothing open to rewind past, and `balance_run_markers_after_retire`
      # deliberately leaves a RUNNING session's marker alone — the stage would
      # then be green over a session it never tested.
      node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" false "$BASH_POLICY" >/dev/null || exit 1
      start_server || exit 1
      drive dangle qa-dangle || RC=1
      hard_kill_server
      [ "$RC" = "0" ] && { start_server || exit 1; }
      [ "$RC" = "0" ] && { drive rewind do || RC=1; }
      hard_kill_server
      [ "$RC" = "0" ] && { start_server || exit 1; }
      [ "$RC" = "0" ] && { drive rewind after || RC=1; }
      if [ "$RC" = "0" ]; then
        "$BIN" resume --json "$(cat "$SESSION_FILE")" >"$RECEIPT" 2>"$QA_ROOT/resume.err"
        echo "resume receipt after the rewind:"; cat "$RECEIPT"
        # Asserted in the driver, by PARSING the receipt: every counter of
        # `ResumeReceipt` is serialised unconditionally (no
        # `skip_serializing_if`), so grepping for one of their keys matches any
        # well-formed receipt — the `no_runs` one included. It also has to be
        # counted by `check()` to sit inside this stage's assertion floor.
        drive rewind receipt "$RECEIPT" || RC=1
      fi
      ;;
    knobs)
      start_server || exit 1
      # The crashing turn carries an explicit per-turn directive for model A.
      # Without one the marker's envelope records `model: None` (the agent's
      # CONFIGURED model is not a routing directive — measured, see
      # `sendTurn`), and this stage would be asserting over a run that has no
      # snapshot to replay.
      # …and an explicit per-turn exec tier, for the second knob. `ask` is the
      # TIGHT end here: the row is opened up to `full` after the crash, so a
      # resume that dropped the ceiling would execute at `full`. The plan for
      # this round wrote the arrangement the other way round (snapshot `full`,
      # session `ask`) — that one is green for a build with no ceiling at all,
      # because the session rung already answers `ask` (判据 #2/#14).
      drive dangle qa-dangle qa-model-a ask || RC=1
      hard_kill_server
      # The session is moved to model B AFTER the crashed run started under A,
      # and with the server DOWN — there is no in-process path to this write
      # from outside (drive_r2.mjs::cmdKnobs carries the three measurements).
      # Its rc counts: if the move did not happen, the assertion after the
      # restart is green for a build that never carried the envelope at all.
      [ "$RC" = "0" ] && { drive knobs pin qa-model-b ask || RC=1; }
      [ "$RC" = "0" ] && { start_server || exit 1; }
      [ "$RC" = "0" ] && { "$BIN" resume --json "$(cat "$SESSION_FILE")" >"$RECEIPT" 2>"$QA_ROOT/resume.err"; cat "$RECEIPT"; }
      [ "$RC" = "0" ] && { drive knobs assert qa-model-a ask || RC=1; }
      ;;
    holes)
      start_server || exit 1
      drive dangle qa-burst || RC=1
      # The burst run must FINISH before the kill: `dangle` returns on the
      # FIRST durable dispatch, and killing there would leave dangling calls
      # whose resume adds a turn's worth of usage — the "billed once"
      # comparison would then be red for a reason that is not the projector's.
      [ "$RC" = "0" ] && { drive holes-settle || RC=1; }
      [ "$RC" = "0" ] && { drive holes "$QA_ROOT/server.log" before || RC=1; }
      hard_kill_server
      [ "$RC" = "0" ] && { start_server || exit 1; }
      [ "$RC" = "0" ] && { drive holes "$QA_ROOT/server.log" after || RC=1; }
      ;;
  esac

  say "mock provider log"; tail -20 "$QA_ROOT/mock.log"
  say "server log tail"; tail -30 "$QA_ROOT/server.log"
  # Only meaningful on a stage that otherwise passed: a stage that failed early
  # legitimately stops asserting, and saying "under-measured" there would bury
  # the failure that actually happened.
  say "assertions: $ASSERTS (floor $FLOOR)"
  if [ "$RC" = "0" ] && [ "$ASSERTS" -lt "$FLOOR" ]; then
    echo "FAIL: stage '$STAGE' passed while asserting only $ASSERTS times, below its measured floor of $FLOOR — a phase stopped asserting" >&2
    RC=1
  fi
  say "verdict: rc=$RC"
  exit "$RC"
fi

# The round-1 stages need a real interpreter, and this host does not have one:
# `python3` and `python` are both the `WindowsApps` stub. Without this probe the
# first Python line below fails with a bare exit code and no sentence, which
# reads as "patch_config.py is broken" — the reader then goes debugging a script
# that was never executed. Probe the interpreter by its OUTPUT, not its exit
# status: the whole reason this stub was mis-described for two rounds is that
# nobody had checked which of the two it gets wrong.
if [ "$(python3 -c 'print("py-ok")' 2>/dev/null)" != "py-ok" ]; then
  echo "stage '$STAGE' is Python and this host has no usable python3 (both python3 and python are the WindowsApps stub: no output, exit 49)." >&2
  echo "The round-2 stages cover this tree and are Node: claims | denied | rewind | knobs | holes" >&2
  exit 78
fi

say "patch config"
python3 "$BUSY/patch_config.py" "$CONFIG" --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1
# `bash` is not idempotent, so the default `auto` tier raises a confirmation
# card and the resumed run would park on a human who is not there. An
# explicit `allow` outranks the tier — the knob an operator would use.
python3 "$PLANH/add_overrides.py" "$CONFIG" bash=allow || exit 1

# The tool call every "tool" turn dispatches. `sleep 120` never returns on
# any timescale this fixture runs on, so it is guaranteed to still be
# in-flight (and therefore dangle) when the server is killed. The field name
# is load-bearing: `BashExecArgs.cmd` (src/builtin_tools/bash_exec.rs), NOT
# `command` — the wrong key deserializes to an EMPTY command under
# `#[serde(default)]`, which returns instantly and never dangles at all.
cat >"$QA_ROOT/tool_spec.json" <<'JSON'
{"name": "bash", "input": {"cmd": "sleep 120"}}
JSON

say "start mock provider (plan channel-burst)"
# `mock_anthropic.py` has no side-channel exemption (unlike `mock_halt.py`):
# EVERY POST it receives advances the turn counter, including calls this
# fixture never asked for — a token-count pre-check that carries the same
# messages as the real call but never becomes a session event, observed to
# precede the real dispatch by 1-3 turns and confirmed by the durable log
# never growing a matching `assistant_message`/`tool_call_requested` for
# them. A short plan ("quick": 2 tool turns then end) falls off its own end
# before the real turn ever lands and the "dangle" is never created — this
# was the fixture's first failure mode on a real machine. "channel-burst"'s
# 16 tool-turn tail absorbs that noise; its own "think" pacing does not gate
# when a request is WRITTEN to $REQUEST_LOG (that happens the instant the
# request arrives, before the mock's simulated think time), which is the
# only thing this fixture ever waits on.
python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" /etc/hostname channel-burst \
  "$QA_ROOT/tool_spec.json" "$REQUEST_LOG" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 20); do
  # GET, not POST: `do_GET` never touches the turn counter, so the readiness
  # probe itself cannot burn one of the plan's slots.
  curl -sf -o /dev/null -m 1 "http://127.0.0.1:$MOCK_PORT/v1/messages" 2>/dev/null && break
  kill -0 "$MOCK_PID" 2>/dev/null || break
  sleep 0.5
done
kill -0 "$MOCK_PID" 2>/dev/null || { echo "mock provider died on startup — port $MOCK_PORT taken?" >&2; tail -5 "$QA_ROOT/mock.log" >&2; exit 70; }

# Send one turn and PROVE the dangling call actually landed durably before
# returning — see drive_dangle.py's `send` mode. A blind sleep here is
# exactly the trap this fixture exists to avoid: guess too short and the
# rest of the run measures nothing. The budget is generous (a real turn can
# land on a 20s-think slot of the plan above, and more than one may precede
# it).
send_and_confirm_dangle() {
  python3 "$HERE/drive_dangle.py" --mode send --port "$GATEWAY_PORT" \
    --channel "gui:qa-resume-boundary" --session-file "$SESSION_FILE" \
    --events-db "$EVENTS_DB" --budget 120
}

# The fixture's own instrument check: prove a dangling call exists in the
# durable log before asserting anything about repair text. If this fails,
# every later assertion in the stage would be passing over an empty set.
assert_dangling() {
  python3 "$HERE/drive_dangle.py" --mode assert-dangling \
    --events-db "$EVENTS_DB" --min-count "$1"
}

set_resume() {
  python3 "$HERE/drive_dangle.py" --mode config-resume --enabled "$1" --config "$CONFIG"
}

case "$STAGE" in
  crash)
    say "crash: dangle -> kill -9 -> restart with resume ON"
    set_resume true
    start_server || exit 1
    send_and_confirm_dangle || { echo "instrument failure: the dangling call was never created" >&2; exit 1; }
    hard_kill_server
    say "assert-dangling (instrument self-check)"
    assert_dangling 1 || exit 1

    start_server || exit 1
    say "waiting for the repaired request to reach the mock"
    if ! wait_for_text "$REQUEST_LOG" "OUTCOME UNKNOWN" 120; then
      echo "FAIL: no repair text ever reached the mock" >&2
      tail -40 "$QA_ROOT/server.log" >&2
      exit 1
    fi
    python3 "$HERE/assert_repairs.py" --request-log "$REQUEST_LOG" --stage crash
    ;;

  attribute)
    say "attribute: dangle with resume OFF, then a second dangle, same session"
    set_resume false
    start_server || exit 1
    send_and_confirm_dangle || { echo "instrument failure: dangle #1 was never created" >&2; exit 1; }
    hard_kill_server
    say "assert-dangling (instrument self-check, dangle #1)"
    assert_dangling 1 || exit 1

    # Restart with resume still OFF: nothing is repaired, dangle #1 survives.
    start_server || exit 1
    send_and_confirm_dangle || { echo "instrument failure: dangle #2 was never created" >&2; exit 1; }
    hard_kill_server
    say "assert-dangling (instrument self-check, dangles #1+#2)"
    assert_dangling 2 || exit 1

    # Now turn resume ON. The boot scan sees TWO dangling calls from TWO
    # separate RunStarted markers in the SAME session.
    set_resume true
    start_server || exit 1
    say "waiting for the repaired request to reach the mock"
    if ! wait_for_text "$REQUEST_LOG" "OUTCOME UNKNOWN" 120; then
      echo "FAIL: no repair text ever reached the mock" >&2
      tail -40 "$QA_ROOT/server.log" >&2
      exit 1
    fi
    python3 "$HERE/assert_repairs.py" --request-log "$REQUEST_LOG" --stage attribute
    ;;
esac
RC=$?

say "mock provider log"
tail -30 "$QA_ROOT/mock.log"

say "server log tail"
LOGDIR="$ALEPH_HOME/logs"
if [ -d "$LOGDIR" ]; then
  tail -40 "$LOGDIR"/aleph-server.log* 2>/dev/null | tail -40
else
  tail -20 "$QA_ROOT/server.log"
fi

say "verdict: rc=$RC"
exit "$RC"
