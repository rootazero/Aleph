#!/usr/bin/env bash
# Real-machine QA for the crash boundary — what a `kill -9` mid-run leaves
# behind, what the restart makes of it, and what the model is then TOLD.
#
#   ./qa/resume_boundary/run.sh claims       # one reduction, three faces (wire / receipt / effect)
#   ./qa/resume_boundary/run.sh denied       # a denied dangle reads NOT EXECUTED, never OUTCOME UNKNOWN
#   ./qa/resume_boundary/run.sh parked       # a dangle parked at a gate reads NEVER RAN
#   ./qa/resume_boundary/run.sh rewind       # a rewind past an open run leaves the marker tail balanced
#   ./qa/resume_boundary/run.sh knobs        # the resumed run follows its RunStarted snapshot
#   ./qa/resume_boundary/run.sh holes        # a burst loses no transcript row and bills once
#   ./qa/resume_boundary/run.sh unanswered   # a crash between the seed and RunStarted is resumed
#   ./qa/resume_boundary/run.sh ratchet      # [resume] max_attempts abandons a crash loop
#   ./qa/resume_boundary/run.sh parallel     # the boot scan fans out max_concurrent at a time
#   ./qa/resume_boundary/run.sh undecodable  # an unreadable row refuses ITS session; ignorable is skipped
#   ./qa/resume_boundary/run.sh attribute    # an EARLIER run's dangle is not blamed on this restart
#   ./qa/resume_boundary/run.sh tombstone    # a background job outlives two boots: still-running, then exited
#   KEEP=1 SKIP_BUILD=1 ./qa/resume_boundary/run.sh <stage>
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
# `attribute` is the falsifying arm for the defect the r2 design spec (§1.4)
# fixed: run it on the pre-round tree and it must FAIL, both dangles
# misattributed to "the server restarted" instead of the older one reading
# "an earlier run in this session". Since r3 it is Node like every other
# stage; the round-1 Python pair (`drive_dangle.py` / `assert_repairs.py`)
# and the `crash` stage are gone — what `crash` proved (a dangling call is
# answered OUTCOME UNKNOWN and the text reaches the model) is the dangle →
# boundary-repair path every stage here walks: `claims` pins its wire and
# receipt faces, `denied` / `parked` pin the sentence the model is handed.
#
# How the dangle is made to happen: the mock answers a `qa-dangle` marker
# with a `bash` tool_use that the `ask` gate parks on a card nobody answers
# (`patch_r2.mjs`'s header, point 3, has the two measurements that rule out a
# long-running command on this host), and the shell kills the server with
# `kill -9` (not SIGTERM: a clean shutdown lets in-flight work settle and
# there would be nothing left to repair) once the durable event log holds
# the dispatch. `drive dangle` does not guess that timing with a sleep; it
# polls the event log itself for the NEW `tool_call_requested` row before
# returning, so the kill lands exactly once the call is durably dangling and
# no earlier. `attribute` needs a call that is in flight rather than parked
# (the fourth repair arm answers a parked one), so its marker (`qa-spawn`)
# dispatches a foreground `subagent` whose child turn the mock holds 120 s.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
STAGE="${1:-claims}"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-resume-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18831}"
MOCK_PORT="${MOCK_PORT:-18832}"

case "$STAGE" in
  claims|denied|rewind|knobs|holes|parked|unanswered|ratchet|parallel|undecodable|attribute|tombstone) ;;
  *) echo "unknown stage: $STAGE (claims|denied|rewind|knobs|holes|parked|unanswered|ratchet|parallel|undecodable|attribute|tombstone)" >&2; exit 64 ;;
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
# `<ALEPH_HOME>/data/sessions.db`. The shell never opens it; the driver
# derives the path from `$QA_ROOT` (`drive_r2.mjs::EVENTS_DB`) and owns every
# read and forge of it.
SESSION_FILE="$QA_ROOT/session_key.txt"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

MOCK_PID=""
SERVER_PID=""
say() { printf '\n=== %s ===\n' "$*"; }

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

# The `BeforeAgentStart` sleepers `drive hooks` installs outlive a `kill -9`
# of the server (nothing on Windows kills a dead process's children), and
# their cwd is `$ALEPH_HOME` — the hook's `plugin_root` — so an orphan that is
# still sleeping when `cleanup` runs keeps `rm -rf "$QA_ROOT"` from finishing.
# They are found by the script name on their command line
# (`drive_r2.mjs::cmdHooks` writes `qa-resume-sleeper.mjs`), never by image
# name (that would take every `node` on the box, this fixture's own drivers
# included).
kill_sleepers() {
  if command -v pkill >/dev/null 2>&1; then
    pkill -f qa-resume-sleeper 2>/dev/null || true
  elif command -v powershell.exe >/dev/null 2>&1; then
    # `-ne $PID`: the enumerating shell's own command line carries the
    # marker too (`pkill -f` excludes itself; this has to say so).
    powershell.exe -NoProfile -Command \
      "Get-CimInstance Win32_Process | Where-Object { \$_.CommandLine -like '*qa-resume-sleeper*' -and \$_.ProcessId -ne \$PID } | ForEach-Object { Stop-Process -Id \$_.ProcessId -Force -ErrorAction SilentlyContinue }" \
      >/dev/null 2>&1 || true
  fi
}

# Take the sleeper hook back out of `$ALEPH_HOME` (the driver owns the file
# list). A kept root re-used for a later stage would otherwise hold every
# turn for the sleep and read as "server slow". Runs at the end of each arm
# that installed it AND from `cleanup`, so an early `exit 1` uninstalls too;
# a no-op on a home that never had it.
uninstall_hook() {
  [ -n "${QA_ROOT_M:-}" ] || return 0
  node "$HERE/drive_r2.mjs" "$GATEWAY_PORT" "$QA_ROOT_M" hooks off >/dev/null 2>&1 || true
}

# The `tombstone` stage's orphan (`sleep 300` in the agent's shell) is NOT a
# sleeper hook — `kill_sleepers` cannot see it — and the server never kills it
# (U5). `drive bg` writes its pid to `job.json`; the green path ends it with
# `drive kill-sleep`, which marks the file `killed_by_fixture`, and a run that
# stopped before that point ends it here. Before `rm -rf "$QA_ROOT"`, since
# the orphan's cwd is inside the root. Skipped once the fixture has killed it:
# a pid the fixture already ended may belong to somebody else by now.
kill_orphan_job() {
  local f="${QA_ROOT_M:-$QA_ROOT}/job.json"
  [ -f "$f" ] || return 0
  node -e '
    const j = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    if (!j.killed_by_fixture && j.pid) { try { process.kill(j.pid, "SIGKILL"); } catch {} }
  ' "$f" >/dev/null 2>&1 || true
}

cleanup() {
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  uninstall_hook
  kill_sleepers
  kill_orphan_job
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  qa_build -p alephcore --bin aleph-server || { echo "build failed" >&2; exit 1; }
fi
# `.cargo/config.toml` pins a shared absolute target dir, so `$REPO/target` is
# wrong from any git worktree — ask cargo, and parse the answer with node.
# Every stage is Node (the guard is right here, not further down: a missing
# `node` used to fall through to a `python3` that on this host is the
# `WindowsApps` stub — prints NOTHING, exits 49, measured 2026-09-03 — so the
# command substitution yielded an empty path and the fixture went looking for
# a binary at `/debug/aleph-server`, a message that reads like a build failure
# and is not one. That fallback is gone with the Python stages in r3.)
command -v node >/dev/null 2>&1 || { echo "node is required" >&2; exit 1; }
META="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null)"
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
  # An operator (or this repo's own build recipe) who pinned a shared target
  # dir built the binary THERE; `cargo metadata` answers with the workspace's
  # default and would send the fixture to an empty directory.
  TARGET_DIR="$CARGO_TARGET_DIR"
else
  TARGET_DIR="$(printf '%s' "$META" | node -e 'let s="";process.stdin.on("data",c=>s+=c).on("end",()=>console.log(JSON.parse(s).target_directory))')"
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
# Every stage is Node (the `node` guard sits above the first use, next to the
# cargo-metadata parse, and says why the Python fallback is gone). The
# round-1 Python stages that used to sit below a guard here were deleted in
# r3 — see the header for what covers `crash` now.
# ---------------------------------------------------------------------------
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
# `tombstone` wants the opposite of a dangle: a job that RUNS and outlives
# the server. Its arm re-patches with the Windows sandbox primitives off.
[ "$STAGE" = "tombstone" ] && BASH_POLICY="allow"
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
# tree that introduced it (the r2 rows on 2026-09-03). Adding an assertion
# raises the number here in the same commit; a number that drops on its own
# is the defect this guard exists for.
case "$STAGE" in
  claims) FLOOR=13 ;;
  denied) FLOOR=5 ;;
  rewind) FLOOR=11 ;;
  knobs)  FLOOR=10 ;;
  holes)  FLOOR=12 ;;
  parked) FLOOR=11 ;;
  unanswered) FLOOR=29 ;;  # measured 2026-09-18 (2+2+13 main flow, 3+6+3 lost-input twin)
  ratchet)    FLOOR=23 ;;  # measured 2026-09-18 (5+5 held boots, 7+6 settled boots)
  parallel)    FLOOR=30 ;;  # measured 2026-09-18 fix round 1: phase 1 cap 2/3 sessions (1 slots + 4 assert-dangling + 11: the fan-out, `max observed 2`) + phase 2 cap 1/2 sessions (1 + 3 + 10: the config wire, `max observed 1`, scanned=5 skipped=3)
  undecodable) FLOOR=30 ;;  # measured 2026-09-18 (3 assert-dangling, 2 forge, 13 refused, 2 mark-ignorable, 10 skipped)
  attribute)   FLOOR=27 ;;  # measured 2026-09-18 (5+5 in-flight, 1 assert-dangling, 16 texts)
  tombstone)   FLOOR=28 ;;  # measured 2026-09-18 (6 bg, 1 bg-alive, 7 tomb still, 5 kill-tomb, 1 kill-sleep, 8 tomb exited)
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
  parked)
    # Resume OFF so the receipt is the only pass. The dangle is the r2
    # instrument (BASH_POLICY=ask parks at the gate); new here: the kill
    # waits for the PARK ROW, not just the dispatch.
    node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" false "$BASH_POLICY" >/dev/null || exit 1
    start_server || exit 1
    drive dangle-parked qa-dangle || { echo "instrument failure: no parked dangle" >&2; RC=1; }
    hard_kill_server
    [ "$RC" = "0" ] && { drive assert-dangling 1 || RC=1; }
    [ "$RC" = "0" ] && { start_server || exit 1; }
    [ "$RC" = "0" ] && { drive parked wire || RC=1; }
    if [ "$RC" = "0" ]; then
      say "aleph-server resume --json"
      "$BIN" resume --json "$(cat "$SESSION_FILE")" >"$RECEIPT" 2>"$QA_ROOT/resume.err"
      echo "resume rc=$? receipt:"; cat "$RECEIPT"
      drive parked model "$RECEIPT" || RC=1
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
    # The deferral count is read off the TRACING log under `$ALEPH_HOME/logs`
    # (the driver knows that path) — NOT `$QA_ROOT/server.log`, which is the
    # process's stdout and never held the `projector queue full` line: passing
    # it here made the count 0 by construction for two rounds (2026-09-18).
    [ "$RC" = "0" ] && { drive holes before || RC=1; }
    hard_kill_server
    [ "$RC" = "0" ] && { start_server || exit 1; }
    [ "$RC" = "0" ] && { drive holes after || RC=1; }
    ;;
  unanswered)
    # §5.2: a crash between the seed (`UserMessage`) and `RunStarted` leaves
    # a marker-less session, and the boot must resume the message anyway.
    # The window is ~30 ms wide on this host; memory is switched ON with the
    # mock as the embedding provider so the query embedding — the only
    # remote call inside that window — stalls 10 s (patch_r2.mjs, point 5).
    # Step 0 is the driver's own assertion, not a thing to eyeball: the
    # stalling embeddings request must fall between the user_message row
    # and the kill, or the stage is an INSTRUMENT FAILURE, never a pass.
    node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" embed-stall >/dev/null || exit 1
    kill -9 "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null
    QA_EMBED_STALL_MS="${QA_EMBED_STALL_MS:-10000}" node "$HERE/mock_r2.mjs" "$MOCK_PORT" "$R2_REQUESTS" >"$QA_ROOT/mock.log" 2>&1 &
    MOCK_PID=$!; sleep 1
    start_server || exit 1
    drive window qa-unanswered || { echo "instrument failure: the seed→RunStarted window never opened" >&2; RC=1; }
    hard_kill_server
    [ "$RC" = "0" ] && { drive unanswered after-kill || RC=1; }
    # The stall-phase mock log is the Step-0 evidence; keep it before the
    # mock is restarted below (its log is truncated on restart).
    cp "$QA_ROOT/mock.log" "$QA_ROOT/mock.stall.log" 2>/dev/null
    # Boot with resume OFF first: the wire face must say `unanswered` on its
    # own, before anything has repaired the session. The mock is restarted
    # without a stall so that every later embedding request answers at once.
    kill -9 "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null
    QA_EMBED_STALL_MS=0 node "$HERE/mock_r2.mjs" "$MOCK_PORT" "$R2_REQUESTS" >"$QA_ROOT/mock.log" 2>&1 &
    MOCK_PID=$!; sleep 1
    node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" false "$BASH_POLICY" embed-stall >/dev/null || exit 1
    [ "$RC" = "0" ] && { start_server || exit 1; drive unanswered before-resume || RC=1; hard_kill_server; }
    # Resume ON: the boot scan must find the marker-less session through the
    # activity window and re-run the message.
    node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" embed-stall >/dev/null || exit 1
    [ "$RC" = "0" ] && { drive boot-mark || RC=1; }
    [ "$RC" = "0" ] && { start_server || exit 1; drive unanswered after-resume || RC=1; hard_kill_server; }
    # §8.2(b), the twin on a SECOND session: a kill inside the
    # `BeforeAgentStart` hook — after the engine wrote its task row, before
    # the orchestrator seeded — leaves a message NO log holds. The boot must
    # tell the user once (a SystemMessage on that session, `notified=1`) and
    # stamp the row (`adjudicated_at_ms`) so the next boot says nothing.
    # The hook is installed with the server DOWN: hooks.json is read at boot.
    [ "$RC" = "0" ] && { drive hooks 120000 || RC=1; }
    [ "$RC" = "0" ] && { start_server || exit 1; drive notice send || RC=1; }
    hard_kill_server
    [ "$RC" = "0" ] && { drive notice after-kill || RC=1; }
    [ "$RC" = "0" ] && { drive boot-mark || RC=1; }
    [ "$RC" = "0" ] && { start_server || exit 1; drive notice first-boot || RC=1; hard_kill_server; }
    [ "$RC" = "0" ] && { drive boot-mark || RC=1; }
    [ "$RC" = "0" ] && { start_server || exit 1; drive notice second-boot || RC=1; }
    # Both on the green and on the RC=1 path (nothing above is gated on RC
    # from here on); `cleanup` repeats them for an early `exit 1`.
    uninstall_hook
    kill_sleepers
    ;;
  ratchet)
    # §5.1: `[resume] max_attempts = 2`. The stamp is written BEFORE the
    # retrigger, so a crash anywhere before the resumed run's own RunStarted
    # counts: boot 1 (0 stamps → stamp #1), boot 2 (1 → #2), boot 3 (2 ≥ 2 →
    # abandon), boot 4 (clean, nothing to do). The crash is aimed with a
    # `BeforeAgentStart` sleeper — the resumed run is admitted, its task row
    # written, and then it waits in the hook until the kill lands.
    #
    # On boots 1 and 2 the boot-scan line can NOT appear: `settle` joins the
    # retrigger, the retrigger awaits the run, the run is inside the hook. So
    # the kill is aimed at the engine's `Agent execution started` line
    # (`drive ratchet-hold`), and boots 1–2 assert the ABSENCE of a scan line
    # for that boot; boots 3–4 wait for theirs.
    QA_MAX_ATTEMPTS=2 node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" >/dev/null || exit 1
    start_server || exit 1
    drive dangle qa-dangle || { echo "instrument failure: no dangle" >&2; RC=1; }
    hard_kill_server
    # Installed AFTER the first crash so the dangle turn itself was not held.
    [ "$RC" = "0" ] && { drive hooks 300000 || RC=1; }
    for n in 1 2 3 4; do
      [ "$RC" = "0" ] || break
      drive boot-mark || { RC=1; break; }
      start_server || exit 1
      if [ "$n" -le 2 ]; then
        drive ratchet-hold "$n" || { echo "boot $n: the resumed run was never admitted" >&2; RC=1; }
        hard_kill_server
      else
        # Nothing to hold: let the scan settle, assert, then stop cleanly.
        drive ratchet "$n" || RC=1
        hard_kill_server
        continue
      fi
      [ "$RC" = "0" ] && { drive ratchet "$n" || RC=1; }
    done
    # Both on the green and on the RC=1 path. Orphaned sleepers
    # self-terminate (300 s); killing them is belt and braces, and it is
    # what lets `rm -rf "$QA_ROOT"` finish on Windows (their cwd is inside).
    uninstall_hook
    kill_sleepers
    ;;
  parallel)
    # §8.1: `[resume] max_concurrent` bounds the boot scan's fan-out, and no
    # candidate is lost to it. Two phases on one log, each: N sessions with
    # one parked dangle apiece, a kill, a re-patched cap, a marked resume-ON
    # boot. The mock holds every `slow`-tagged resumed run's END for
    # `QA_SLOW_MS`, so runs admitted together overlap for seconds; the driver
    # samples the run registry through `gateway.metrics.run_concurrency` at
    # 150 ms and asserts the max it saw is EXACTLY the cap.
    #
    #   phase 1 — cap 2 over 3 sessions: the FAN-OUT. Two resumed runs really
    #     are in flight at once (1 is the T15 semaphore mutation, or an overlap
    #     shorter than the poll — widen `QA_SLOW_MS`, never the equality) and
    #     the third is not lost (3 is the cap not honoured; `resumed=3`).
    #   phase 2 — cap 1 over 2 sessions: the WIRE. `[resume] max_concurrent`
    #     DEFAULTS to 2 (T15 lowered it after the brief was written), so a cap
    #     of 2 has no red state for config → semaphore: a patcher that never
    #     wrote the key leaves the default and phase 1 stays green. Cap 1 is
    #     the one value below the default, so "the key was not read" is
    #     `max observed 2` here, by name. Phase 2 runs on the same log after
    #     phase 1's three sessions settled, so its scan visits them too and
    #     files them `skipped`; the driver is told how many (`clean`).
    #
    # Each phase's cap is spelled once, on its patch line, and handed to the
    # driver from the same variable. Before each dangle loop the driver asks
    # the engine whether it can hold that many parked runs on one agent (a
    # parked dangle holds a run slot; `max_runs_per_agent` defaults to 3, so
    # phase 1 sits exactly at it) — the fast, named form of the 180 s
    # INSTRUMENT FAILURE a queued dangle would otherwise produce. The sessions
    # are EPOCHS of the main key (`agent:main:main:s1..s5`): the Main grammar
    # is `agent:<id>:main[:sN]`, and a third segment that is not `main` parses
    # as a Task session (measured 2026-09-18: `agent:main:qa-a:s1` landed as
    # `{"type":"task","task_type":"qa-a","task_id":"s1"}`).
    P1_CAP=2
    QA_MAX_CONCURRENT="$P1_CAP" node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" >/dev/null || exit 1
    start_server || exit 1
    drive parallel-slots 3 || RC=1
    P1_KEYS=""
    for n in 1 2 3; do
      [ "$RC" = "0" ] || break
      k="agent:main:main:s$n"
      P1_KEYS="$P1_KEYS $k"
      drive dangle "qa-dangle:slow-$n" "" "" "$k" || { echo "instrument failure: no dangle on $k" >&2; RC=1; }
    done
    hard_kill_server
    # shellcheck disable=SC2086  # the key lists are deliberately word-split
    [ "$RC" = "0" ] && { drive assert-dangling 3 $P1_KEYS || RC=1; }
    [ "$RC" = "0" ] && { drive boot-mark || RC=1; }
    [ "$RC" = "0" ] && { start_server || exit 1; }
    # shellcheck disable=SC2086
    [ "$RC" = "0" ] && { drive parallel "$P1_CAP" 0 $P1_KEYS || RC=1; }
    # Phase 2 dangles on the server phase 1 left running (its three resumed
    # runs have finished, so the slots are free); the cap is re-patched with
    # the server DOWN, like every other config change in this file.
    [ "$RC" = "0" ] && { drive parallel-slots 2 || RC=1; }
    P2_KEYS=""
    for n in 4 5; do
      [ "$RC" = "0" ] || break
      k="agent:main:main:s$n"
      P2_KEYS="$P2_KEYS $k"
      drive dangle "qa-dangle:slow-$n" "" "" "$k" || { echo "instrument failure: no dangle on $k" >&2; RC=1; }
    done
    hard_kill_server
    P2_CAP=1
    [ "$RC" = "0" ] && { QA_MAX_CONCURRENT="$P2_CAP" node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" >/dev/null || exit 1; }
    # shellcheck disable=SC2086
    [ "$RC" = "0" ] && { drive assert-dangling 2 $P2_KEYS || RC=1; }
    [ "$RC" = "0" ] && { drive boot-mark || RC=1; }
    [ "$RC" = "0" ] && { start_server || exit 1; }
    # shellcheck disable=SC2086
    [ "$RC" = "0" ] && { drive parallel "$P2_CAP" 3 $P2_KEYS || RC=1; }
    ;;
  undecodable)
    # §4.5 / T14: a row this build cannot decode refuses ITS session — under
    # its own tag, on the attach face, in the coordinator's log and in the
    # doctor — and no other session; the same row marked `ignorable: true` by
    # its writer is skipped, counted by the doctor, and refuses nothing. Two
    # sessions, one parked dangle each. The forge appends to `x` with the
    # server DOWN: the store is the only writer of that table while it runs,
    # and a second one would race its seq. Each resume-ON boot is marked so
    # the driver reads THAT boot's scan line and refusal lines. Two epochs of
    # the main key, for the grammar reason `parallel` states.
    U_X="agent:main:main:s1"
    U_Y="agent:main:main:s2"
    node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true "$BASH_POLICY" >/dev/null || exit 1
    start_server || exit 1
    drive dangle qa-dangle:x "" "" "$U_X" || { echo "instrument failure: no dangle on $U_X" >&2; RC=1; }
    [ "$RC" = "0" ] && { drive dangle qa-dangle:y "" "" "$U_Y" || { echo "instrument failure: no dangle on $U_Y" >&2; RC=1; }; }
    hard_kill_server
    [ "$RC" = "0" ] && { drive assert-dangling 2 "$U_X" "$U_Y" || RC=1; }
    [ "$RC" = "0" ] && { drive undecodable forge "$U_X" || RC=1; }
    [ "$RC" = "0" ] && { drive boot-mark || RC=1; }
    [ "$RC" = "0" ] && { start_server || exit 1; }
    [ "$RC" = "0" ] && { drive undecodable refused "$U_X" "$U_Y" || RC=1; }
    hard_kill_server
    [ "$RC" = "0" ] && { drive undecodable mark-ignorable "$U_X" || RC=1; }
    [ "$RC" = "0" ] && { drive boot-mark || RC=1; }
    [ "$RC" = "0" ] && { start_server || exit 1; }
    [ "$RC" = "0" ] && { drive undecodable skipped "$U_X" "$U_Y" || RC=1; }
    ;;
  attribute)
    # §1.4: two dangles from two crashes in ONE session — resume OFF between
    # them, so dangle #1 survives the restart — repaired by one resume-ON
    # boot, must read two different sentences. The dangle is a foreground
    # `subagent` whose child turn the mock holds (`qa-spawn`): after §6.1 a
    # call parked at the `ask` gate is answered by the fourth arm, whose
    # wording is not the one this stage asserts, so the call has to be
    # genuinely in flight. `in-flight` after each kill pins that: the
    # dispatch names `subagent`, no `tool_call_parked` row, one open
    # `RunStarted` per crash.
    node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" false allow >/dev/null || exit 1
    start_server || exit 1
    drive dangle qa-spawn:1 || { echo "instrument failure: dangle #1 was never created" >&2; RC=1; }
    hard_kill_server
    [ "$RC" = "0" ] && { drive attribute in-flight 1 || RC=1; }
    # Resume still OFF: nothing is repaired, dangle #1 survives.
    [ "$RC" = "0" ] && { start_server || exit 1; }
    [ "$RC" = "0" ] && { drive dangle qa-spawn:2 || { echo "instrument failure: dangle #2 was never created" >&2; RC=1; }; }
    hard_kill_server
    [ "$RC" = "0" ] && { drive assert-dangling 2 || RC=1; }
    [ "$RC" = "0" ] && { drive attribute in-flight 2 || RC=1; }
    # Resume ON: one boot scan sees TWO dangling calls from TWO RunStarted
    # markers in the SAME session.
    node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true allow >/dev/null || exit 1
    [ "$RC" = "0" ] && { drive boot-mark || RC=1; }
    [ "$RC" = "0" ] && { start_server || exit 1; }
    [ "$RC" = "0" ] && { drive attribute texts || RC=1; }
    ;;
  tombstone)
    # U5 / §9: a background `bash` job (`sleep 300`, `qa-bg`) is a real OS
    # process. Boot 1 spawns it and the server is killed with `kill -9` —
    # TerminateProcess, no reaper, the child survives. Boot 2's reconcile
    # finds the `running` row, asks the OS about its pid (+ creation time,
    # against pid reuse) and writes `still_running_unattached`; a poll hands
    # the model that word, the pid and the stop command, and a `kill` runs
    # NOTHING (the fixture asks the OS whether the orphan is alive — it is).
    # The FIXTURE then kills the orphan, the server is killed again, and boot
    # 3 RE-ASKS the still-running row (the reconcile does that on every boot)
    # and turns it into `exited_during_restart` without deleting anything.
    #
    # `allow`: the job must actually run. Windows sandbox primitives off
    # (`QA_WINDOWS_SANDBOX_OFF`, patch_r2.mjs point 7): with them on,
    # `child.id()` is the `sandbox-init-windows` launcher and the server's
    # death closes a KILL_ON_JOB_CLOSE job, so the "still running" arm is
    # unreachable. `drive bg` re-measures whether the shell can sleep at all
    # on this host; if the job settles within 2 s the stage is exit 78 —
    # instrument unavailable, recorded as UNRUN, never as a pass.
    QA_WINDOWS_SANDBOX_OFF=1 node "$HERE/patch_r2.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" true allow >/dev/null || exit 1
    start_server || exit 1
    drive bg; rc=$?
    [ "$rc" = "78" ] && exit 78
    [ "$rc" = "0" ] || RC=1
    hard_kill_server
    [ "$RC" = "0" ] && { drive bg-alive yes || RC=1; }
    [ "$RC" = "0" ] && { start_server || exit 1; }
    [ "$RC" = "0" ] && { drive tomb still a || RC=1; }
    [ "$RC" = "0" ] && { drive kill-tomb || RC=1; }
    [ "$RC" = "0" ] && { drive kill-sleep || RC=1; }
    hard_kill_server
    [ "$RC" = "0" ] && { start_server || exit 1; }
    [ "$RC" = "0" ] && { drive tomb exited b || RC=1; }
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
