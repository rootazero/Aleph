#!/usr/bin/env bash
# Real-machine QA for the embedded terminal's agent panel (FEATURE_LOCATOR
# §6.11 / §6.12) — the foreground probe, the state sampler, the quiet clock,
# the merged cwd, and the `terminal` tool face, driven against a REAL booted
# `aleph-server`.
#
#   ./qa/terminal/run.sh identify   # ~50 s  the probe names the program and the
#                                   #        manifest names the agent, for a
#                                   #        session spawned as `sh`
#   ./qa/terminal/run.sh wait       # ~60 s  terminal{wait} blocks and returns
#                                   #        `reached`; a state that never comes
#                                   #        answers `timeout`
#   ./qa/terminal/run.sh quiet      # ~90 s  30 s of silence publishes
#                                   #        `quiet_since` and does NOT move
#                                   #        `state` (spec R2-3)
#   ./qa/terminal/run.sh cwd        # ~40 s  OSC 7 › foreground probe › spawn dir
#   ./qa/terminal/run.sh all        # every stage in turn, one server each
#
#   KEEP=1 ./qa/terminal/run.sh cwd        # keep the scratch dir for post-mortem
#   SKIP_BUILD=1 ./qa/terminal/run.sh cwd  # reuse the binary already built HERE
#
# ## Why a booted server, and not one more unit test
#
# Phase 1 shipped an agent panel that never identified an agent in production.
# `gateway::runtime` was handed `PtySession::shell` — the SPAWN-TIME label — and
# a Panel terminal sends `{rows, cols}` with no `command`, so the label was
# always `zsh`; `identify_agent("zsh")` answered `None`, the detection engine
# early-returned `Unknown` before consulting a single rule, and twenty-one
# manifests plus every test around them stayed green. Each of those tests
# passes the agent's name in itself, which is exactly why none of them could
# see it (判据 §2 — ask when the thing can go red, not whether it is correct).
#
# This fixture spawns `sh` and TYPES the agent afterwards. The only thing that
# can turn that into `agent: "claude"` is the foreground probe reading the
# process table of a real PTY on a real kernel, which is the one part no
# in-process test reaches.
#
# ## What it deliberately does NOT prove
#
# * The spawn directory as the third cwd tier. Reaching it needs a probe that
#   FAILS, which cannot be arranged from the wire; the `cwd` stage shows only
#   that the spawn dir is the answer neither session gave.
# * `program: null`. Same reason — "the probe could not look" is a platform or
#   permission condition, not something a client can ask for.
# * Anything the Panel RENDERS. Every assertion here is an RPC round trip.
# * The 21 manifests. One agent (`claude`) is exercised end to end; the other
#   twenty are covered in-process by `agent_detect`'s own suite, and a fixture
#   that painted twenty screens would be re-testing the rule engine through the
#   slowest possible instrument.
set -uo pipefail

STAGE="${1:-identify}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"

case "$STAGE" in
  identify|wait|quiet|cwd) ;;
  all)
    RC=0
    for s in identify wait quiet cwd; do
      "$HERE/run.sh" "$s" || RC=1
      # Only the first stage needs to pay for the build; the rest reuse it.
      export SKIP_BUILD=1
    done
    echo; echo "=== all stages: rc=$RC ==="
    exit "$RC"
    ;;
  *) echo "unknown stage: $STAGE (identify|wait|quiet|cwd|all)" >&2; exit 64 ;;
esac

QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-terminal-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18841}"
MOCK_PORT="${MOCK_PORT:-18842}"

# Build BEFORE HOME is redirected: cargo's registry, git cache and rustup
# toolchain all live under the real HOME, and a build launched with the scratch
# one silently degrades into a full network fetch.
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"

say() { printf '\n=== %s ===\n' "$*"; }
SERVER_PID=""
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  # The fake agent holds for a day on purpose (an exited child reads as
  # `visible_idle`), so a killed server would otherwise leave one per stage.
  # Matched on the mktemp SUFFIX, not on `$QA_ROOT/bin/claude`: the fixture
  # canonicalises that path (`/var` -> `/private/var` on macOS) before handing
  # it to the server, so the literal `$QA_ROOT` spelling appears in no command
  # line and the pkill would silently match nothing.
  pkill -f "$(basename "$QA_ROOT")/bin/claude" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

# `pwd -P`, not the string we assembled: on macOS `$TMPDIR` is under a symlink
# (`/var` -> `/private/var`), `jail::resolve_spawn_cwd` canonicalises what it
# admits, and the process table reports canonical paths too. The `cwd` stage
# compares three of these strings for equality, so a fixture holding the
# uncanonicalised spelling would fail every one of them while the product was
# right.
mkdir -p "$QA_ROOT/work" "$QA_ROOT/bin"
WORK="$(cd "$QA_ROOT/work" && pwd -P)"
BIN_DIR="$(cd "$QA_ROOT/bin" && pwd -P)"
mkdir -p "$WORK/spawn" "$WORK/probe" "$WORK/probe2" "$WORK/osc"

say "install the fake agent"
# The NAME is the mechanism: `agent_detect::lookup_agent` resolves by basename,
# so this file only identifies as an agent once it is called `claude`.
cp "$HERE/fake-claude" "$BIN_DIR/claude"
chmod +x "$BIN_DIR/claude"
python3 "$HERE/derive_chrome.py" \
  "$REPO/crates/agent-detect/src/manifests/claude.toml" "$BIN_DIR" || exit 1
echo "  fake agent: $BIN_DIR/claude"

say "build ($STAGE)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  qa_build --bin aleph-server || { echo "build failed" >&2; exit 1; }
fi
# Ask cargo where its target dir really is rather than assuming `$REPO/target`:
# `.cargo/config` pins one ABSOLUTE path shared by every worktree, so the guess
# is wrong from any of them and the binary sitting there can be from a
# different tree entirely.
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }
# The one trace a swallowed build failure leaves behind. See qa/lib/build.sh.
echo "binary: $BIN ($(date -r "$BIN" '+%Y-%m-%d %H:%M:%S'))"
echo "worktree: $REPO"

say "generate a baseline config"
# `--port` on the GENERATION boot, which the older fixtures omit. The config
# does not exist yet, so this boot binds the built-in default port — and if
# anything else on the machine already holds it (another QA run, a developer's
# own daemon) the process exits before writing a config at all. The symptom is
# `no config generated at …`, which reads like a permissions or path problem;
# the cause is one line further down the log. Binding the port this run is
# about to use instead makes the two failures the same failure.
timeout 25 "$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
# The inline provider key is what keeps `tools.invoke` real: with no key the
# server boots `Mode: Simulated`, where `tools.catalog` answers normally and
# `tools.invoke` replies "boot phase 2" — which reads like a missing
# registration and is not.
python3 "$BUSY/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1
python3 "$HERE/patch_terminal.py" "$CONFIG" "$WORK" || exit 1

say "start server"
"$BIN" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
UP=0
for _ in $(seq 1 90); do
  if curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null; then UP=1; break; fi
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
# "I could not ask" must never render as "the answer is no".
[ "$UP" = "1" ] || { echo "HARNESS_GATEWAY_NEVER_CAME_UP"; tail -40 "$QA_ROOT/server.log"; exit 1; }
echo "gateway up on $GATEWAY_PORT"
# The memory this fixture would otherwise re-learn: `Mode: Simulated` makes
# `tools.invoke` answer like a config error.
grep -m1 "Mode:" "$QA_ROOT/server.log" || echo "  (no Mode: line in the server log)"

say "drive: $STAGE"
RC=0
python3 "$HERE/drive_terminal.py" \
  "ws://127.0.0.1:$GATEWAY_PORT/ws" "$STAGE" "$BIN_DIR" "$WORK" "$BIN_DIR/chrome.json" || RC=$?

say "server log tail"
LOGDIR="$ALEPH_HOME/logs"
if [ -d "$LOGDIR" ]; then
  tail -30 "$LOGDIR"/aleph-server.log* 2>/dev/null | tail -30
else
  tail -20 "$QA_ROOT/server.log"
fi

say "verdict: $STAGE rc=$RC"
exit "$RC"
