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
#   ./qa/terminal/run.sh real       # ~60 s  a REAL agent binary off PATH, run
#                                   #        directly and again behind a REAL
#                                   #        `npx`; SKIPS loudly if none
#   ./qa/terminal/run.sh panel      #        boots, sets the board and WAITS —
#                                   #        the browser checklist (tabs / row
#                                   #        click / paste / cursor). Needs
#                                   #        `just wasm` and a Chrome you drive.
#   ./qa/terminal/run.sh all        # every NON-interactive stage in turn
#
# ## Where it runs
#
# The four automated stages (identify / wait / quiet / cwd) and the `panel`
# setup run on Unix AND on Windows. They were Unix-only until 2026-09-05 — not
# by design but by accident of language: the drivers were Python and this
# Windows host has no interpreter, so the whole fixture was UNRUN there.
# Windows is the platform whose foreground probe has no `tcgetpgrp`, i.e. the
# one where `foreground_fact_for_shell` is the WHOLE answer — so "the fixture
# cannot run there" was the most expensive place for it not to run.
#
# ⚠️ Python — the accurate version, because the first draft of this
# paragraph got it wrong and a wrong reason in a comment is the expensive kind
# (判据 §1): this host has NO interpreter installed. `python`/`python3` on
# PATH are WindowsApps stubs that exit 49 without running, and `uv` — which IS
# installed, and is what Aleph's own `bootstrap-runtime` provisions — reports
# none managed yet (`uv python find` exits 2). `uv python install` would fetch
# one, and it would still not unblock `real`/`tui`: CPython ships `pty` only
# on Unix.
#
# `real` and `tui` are Unix-only for that last reason, structurally and
# whatever interpreter is around. They SKIP loudly rather than pass.
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
# * Anything the Panel RENDERS — for the four automated stages. Every
#   assertion in them is an RPC round trip. `panel` is the answer to that: it
#   sets the board and hands it to a browser, because a tab title, a click
#   handler, a browser paste and a cursor painted on a `<canvas>` are not
#   reachable from the wire.
# * The 21 manifests. One agent (`claude`) is exercised end to end; the other
#   twenty are covered in-process by `agent_detect`'s own suite, and a fixture
#   that painted twenty screens would be re-testing the rule engine through the
#   slowest possible instrument.
#
# ## Why `real` exists beside `identify`
#
# `identify` types `claude` into a shell, and that `claude` is
# `fake-claude.cjs` installed as an extensionless `claude` — a Node script
# whose NAME is the mechanism. It covers exactly one arm of
# `normalized_program_name`, the one a stand-in can cover by construction. The
# arms it cannot reach belong to real installs: a `#!/usr/bin/env node` CLI
# the kernel calls `node`, a CLI that rewrites `process.title` (so `argv[0]`
# is the title and macOS bleeds the ENVIRONMENT in after it), and a launcher
# that stays the pgrp leader while the agent runs as its child. Measured here
# on 2026-09-05: `npx pi` leaves the leader as `npm exec pi …` with the real
# `pi` as its child, and one exported variable whose value contains spaces
# scatters bare words into the command line.
set -uo pipefail

STAGE="${1:-identify}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"

case "$STAGE" in
  identify|wait|quiet|cwd|real|panel|tui) ;;
  all)
    RC=0
    # `panel` is deliberately absent: it boots and WAITS for a browser.
    for s in identify wait quiet cwd real tui; do
      "$HERE/run.sh" "$s" || RC=1
      # Only the first stage needs to pay for the build; the rest reuse it.
      export SKIP_BUILD=1
    done
    echo; echo "=== all stages: rc=$RC ==="
    exit "$RC"
    ;;
  *) echo "unknown stage: $STAGE (identify|wait|quiet|cwd|real|panel|tui|all)" >&2; exit 64 ;;
esac

# ---------------------------------------------------------------------------
# Platform, and how this fixture finds a Python
# ---------------------------------------------------------------------------
#
# Everything this fixture drives moved from Python to Node on 2026-09-05,
# because every stage here was UNRUN on Windows — the platform whose foreground
# probe has no `tcgetpgrp` and therefore the only platform where
# `foreground_fact_for_shell` is the WHOLE answer.
#
# Two stages did NOT move, and the reason is structural rather than a porting
# gap: `probe_alive.py` and `drive_tui.py` both drive a program through
# `pty.fork`, which CPython only has on Unix and which Node has no equivalent
# for without a native module. So they are Unix-only WHATEVER interpreter is
# available, and they SKIP loudly where they cannot run — a stage that cannot
# run must never report a pass (判据 §2).
case "$(uname -s 2>/dev/null || echo unknown)" in
  MINGW*|MSYS*|CYGWIN*|Windows_NT) IS_WINDOWS=1 ;;
  *) IS_WINDOWS=0 ;;
esac

# ⚠️ NOT a bare `python3`. This repo's own runtime bootstrap installs `uv`
# (`aleph-server bootstrap-runtime`'s `DEFAULT_TARGETS`) and the model-facing
# prompt steers `uv run` over a bare interpreter
# (`orchestrator/harness_bridge/prompt_build.rs`), so a fixture that assumes a
# SYSTEM interpreter is assuming something Aleph itself does not.
#
# Two traps this resolution exists for:
#   * `python3` on a Windows PATH is usually the WindowsApps Store stub. It is
#     on PATH and `command -v` finds it, so presence is not the test — and
#     ⚠️ **neither is running it**: the stub is an AppExecLink whose entire
#     behaviour is to OPEN THE MICROSOFT STORE on the Python installer page,
#     then exit 49. A probe that "just tries it" pops a Store window on every
#     single invocation of this fixture. Measured 2026-09-05, the hard way.
#     So the stub is recognised WITHOUT being executed: an AppExecLink is a
#     zero-byte reparse point under `.../Microsoft/WindowsApps/`, and either
#     of those two facts is enough to disqualify it.
#   * a machine can have `uv` and still have no interpreter (`uv python find`
#     exits 2). `uv run` would then DOWNLOAD one, and a fixture that quietly
#     fetches a runtime mid-run is its own hazard, so this refuses and prints
#     the command instead of deciding for the operator.
#
# Both traps are the same shape: **the cheap way to ask "can I use this?" has a
# side effect on the operator's machine.** Ask something inert first.
real_interpreter() {
  local p
  p="$(command -v "$1" 2>/dev/null)" || return 1
  [ -n "$p" ] || return 1
  case "$p" in
    *[Ww]indows[Aa]pps*) return 1 ;;
  esac
  # A 0-byte executable is an AppExecLink, not a program.
  [ -s "$p" ] || return 1
  "$p" -c "" >/dev/null 2>&1 || return 1
  printf '%s' "$p"
}

PY_CMD=()
if [ -n "${PY:-}" ]; then
  read -r -a PY_CMD <<<"$PY"
elif PY_REAL="$(real_interpreter python3)"; then
  PY_CMD=("$PY_REAL")
elif PY_REAL="$(real_interpreter python)"; then
  PY_CMD=("$PY_REAL")
elif command -v uv >/dev/null 2>&1 && uv python find >/dev/null 2>&1; then
  PY_CMD=(uv run --no-project python)
fi
run_py() {
  [ "${#PY_CMD[@]}" -gt 0 ] || return 127
  "${PY_CMD[@]}" "$@"
}

if [ "$STAGE" = "real" ] || [ "$STAGE" = "tui" ]; then
  if ! run_py -c "import pty" >/dev/null 2>&1; then
    echo
    echo "=== SKIP: $STAGE ==="
    if [ "${#PY_CMD[@]}" -eq 0 ]; then
      echo "  No Python interpreter here. \`python3\` on PATH is a WindowsApps"
      echo "  AppExecLink — a 0-byte stub that opens the Microsoft Store — and"
      echo "  \`uv python find\` reports none managed. This fixture does NOT run"
      echo "  either of them to find that out."
      echo "  Provision one the way Aleph does:  uv python install 3.12"
    else
      echo "  \`${PY_CMD[*]} -c 'import pty'\` fails here — CPython only ships"
      echo "  \`pty\` on Unix."
    fi
    echo "  This stage drives a program through a pty (probe_alive.py /"
    echo "  drive_tui.py); Node has no pty without a native module, so the"
    echo "  2026-09-05 port could not take these two with it."
    echo "  THIS STAGE ASSERTED NOTHING. It is not a pass."
    exit 0
  fi
fi

# One field out of the panel board. Was five `python3 -c` one-liners.
board_field() {
  node -e 'const fs=require("node:fs");try{const v=JSON.parse(fs.readFileSync(process.argv[1],"utf8"))[process.argv[2]];console.log(v==null?"?":v)}catch{console.log("?")}' \
    "$BOARD" "$1" 2>/dev/null || echo "?"
}

QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-terminal-XXXXXX")}"
# Mixed-form root on Windows: the native server reads `C:/…`, not `/c/…`, and
# HOME / ALEPH_HOME are derived from this below and handed to it as environment.
# Same line as qa/agents_viz, qa/teamchat_rooms, qa/rooms_channel_bind.
command -v cygpath >/dev/null 2>&1 && QA_ROOT="$(cygpath -m "$QA_ROOT")"
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
  if command -v pkill >/dev/null 2>&1; then
    pkill -f "$(basename "$QA_ROOT")/bin/claude" 2>/dev/null
  elif [ "$IS_WINDOWS" = "1" ]; then
    # No pkill here. The fake holds for a day, so leaving one per stage would
    # accumulate; taskkill by image name would take the operator's own node
    # processes, so match on the command line instead.
    powershell -NoProfile -Command \
      "Get-CimInstance Win32_Process | Where-Object { \$_.CommandLine -like '*$(basename "$QA_ROOT")*claude*' } | ForEach-Object { Stop-Process -Id \$_.ProcessId -Force -ErrorAction SilentlyContinue }" \
      >/dev/null 2>&1 || true
  fi
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
# `pwd -P` for the macOS symlink above; then MIXED FORM on Windows, because
# every use of these two below hands them to the NATIVE server —
# `pty.spawn`'s `cwd`, `[agents.defaults] workspace_root`, and the three
# directories the `cwd` stage compares for equality. A `/c/…` spelling is
# refused by `jail::resolve_spawn_cwd` as "outside every registered
# workspace", which reads like a fixture path bug and would be one.
native_dir() {
  local p
  p="$(cd "$1" && pwd -P)"
  command -v cygpath >/dev/null 2>&1 && p="$(cygpath -m "$p")"
  printf '%s' "$p"
}
WORK="$(native_dir "$QA_ROOT/work")"
BIN_DIR="$(native_dir "$QA_ROOT/bin")"
mkdir -p "$WORK/spawn" "$WORK/probe" "$WORK/probe2" "$WORK/osc"

say "install the fake agent"
# The NAME is the mechanism: `agent_detect::lookup_agent` resolves by basename,
# so this file only identifies as an agent once it is called `claude` — with NO
# extension, on both platforms, so `program` reads the same on both wires. See
# the header of fake-claude.cjs.
cp "$HERE/fake-claude.cjs" "$BIN_DIR/claude"
chmod +x "$BIN_DIR/claude" 2>/dev/null || true
# The interpreter is named by ABSOLUTE PATH everywhere below, never as the bare
# word `node`. Measured 2026-09-05: the shim's first version said `node`, the
# shim resolved fine, and the PTY child answered
# `'node' 不是内部或外部命令` — this host's node is an fnm per-shell shim whose
# directory is not on the PATH the server hands its children. A fixture whose
# subject is the foreground probe must not also be testing whether the server
# forwards an environment (判据: one subject per assertion).
QA_NODE="$(command -v node)" || { echo "no node on PATH" >&2; exit 1; }
command -v cygpath >/dev/null 2>&1 && QA_NODE="$(cygpath -m "$QA_NODE")"
export QA_NODE
echo "  node: $QA_NODE"
if [ "$IS_WINDOWS" = "1" ]; then
  # cmd.exe cannot exec a shebang, and PATHEXT is how `claude` typed at a
  # prompt resolves to anything at all. The shim runs IN cmd.exe (a batch file
  # gets no process of its own) and starts node as its CHILD — so on Windows
  # the agent sits one level deeper than on Unix, which is precisely the tree
  # `foreground::foreground_fact_for_shell` has to walk.
  printf '@echo off\r\n"%s" "%%~dp0claude" %%*\r\n' "$QA_NODE" >"$BIN_DIR/claude.cmd"
  echo "  windows shim: $BIN_DIR/claude.cmd"
fi
node "$HERE/derive_chrome.mjs" \
  "$REPO/crates/agent-detect/src/manifests/claude.toml" "$BIN_DIR" || exit 1
echo "  fake agent: $BIN_DIR/claude"

if [ "$STAGE" = "panel" ]; then
  # A debug server serves `interfaces/webchat/dist/` from disk. An empty dist
  # serves a blank page and every checklist item "fails" for the wrong reason,
  # so refuse rather than let the operator debug the fixture.
  DIST="$REPO/interfaces/webchat/dist"
  if [ ! -s "$DIST/index.html" ]; then
    echo "no Panel build at $DIST — run \`just wasm\` first" >&2
    exit 1
  fi
  echo "panel dist: $DIST (index.html $(date -r "$DIST/index.html" '+%Y-%m-%d %H:%M:%S'))"
fi

say "build ($STAGE)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  qa_build --bin aleph-server || { echo "build failed" >&2; exit 1; }
fi
# Ask cargo where its target dir really is rather than assuming `$REPO/target`:
# `.cargo/config` pins one ABSOLUTE path shared by every worktree, so the guess
# is wrong from any of them and the binary sitting there can be from a
# different tree entirely.
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | node -e 'let s="";process.stdin.on("data",c=>s+=c).on("end",()=>console.log(JSON.parse(s).target_directory))')"
BIN="$TARGET_DIR/debug/aleph-server"
[ -x "$BIN" ] || BIN="$TARGET_DIR/debug/aleph-server.exe"
[ -x "$BIN" ] || { echo "no binary at $TARGET_DIR/debug/aleph-server[.exe]" >&2; exit 1; }
# The one trace a swallowed build failure leaves behind. See qa/lib/build.sh.
echo "binary: $BIN ($(date -r "$BIN" '+%Y-%m-%d %H:%M:%S'))"
# Remember exactly which bytes this run is about to execute. A cargo
# build/check/clippy running concurrently shares `target/`, and it will swap
# this file out MID-RUN — the fixture then fails with something like
# `Method not found: runtime.agents.list`, which reads exactly like the
# handler was never registered, i.e. like the defect this fixture exists to
# catch. Measured once: a concurrent `cargo clippy --workspace --all-targets`
# restored an OLDER artifact whose boot log printed `rows_filled=` where this
# tree's `projection_reconciler` prints `holes_filled=`.
#
# Pure observation — it changes nothing, it only refuses to let the swap be
# read as a product defect (判据 §8: "I could not ask" is not "the answer is
# no").
BIN_STAMP="$(date -r "$BIN" '+%s')-$(wc -c <"$BIN" | tr -d ' ')"
echo "worktree: $REPO"

if [ "$STAGE" = "tui" ]; then
  # `aleph-tui` is a DIFFERENT crate, so `--bin aleph-server` never builds it.
  # Built when it is missing even under SKIP_BUILD, because `all` exports
  # SKIP_BUILD=1 after its first stage — so honouring it blindly would make
  # `all` fail on a binary it was never given a chance to produce, with a
  # message about a path.
  TUI_BIN="$TARGET_DIR/debug/aleph-tui"
  [ -x "$TUI_BIN" ] || TUI_BIN="$TARGET_DIR/debug/aleph-tui.exe"
  if [ "${SKIP_BUILD:-0}" != "1" ] || [ ! -x "$TUI_BIN" ]; then
    qa_build -p aleph-tui --bin aleph-tui || { echo "tui build failed" >&2; exit 1; }
    TUI_BIN="$TARGET_DIR/debug/aleph-tui"
    [ -x "$TUI_BIN" ] || TUI_BIN="$TARGET_DIR/debug/aleph-tui.exe"
  fi
  [ -x "$TUI_BIN" ] || { echo "no aleph-tui at $TARGET_DIR/debug" >&2; exit 1; }
  echo "tui binary: $TUI_BIN ($(date -r "$TUI_BIN" '+%Y-%m-%d %H:%M:%S'))"
fi

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
node "$HERE/patch_config.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" "$WORK" || exit 1

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

if [ "$STAGE" = "real" ] || [ "$STAGE" = "panel" ] || [ "$STAGE" = "tui" ]; then
  say "find a real agent binary"
  # The roster is DERIVED from engine.rs, not written here: `agent_label` and
  # `interactive_agent_executable` disagree for four agents (`agy`,
  # `copilot`, `cursor-agent`, `kiro-cli`), so a hand list would be wrong on
  # the day it was written (判据 §1).
  TRIED=""
  export QA_REAL_AGENT="" QA_REAL_AGENT_NAME="" QA_REAL_NPX=""
  # If the derivation itself breaks, the loop below reads nothing and the
  # stage skips with an empty "tried" list — which looks exactly like "no
  # agent is installed" (判据 §8). Say which it is.
  ROSTER="$(node "$HERE/derive_agent_bins.mjs" "$REPO/crates/agent-detect/src/engine.rs" || true)"
  if [ -z "$ROSTER" ]; then
    echo "  derive_agent_bins.mjs produced NO roster — engine.rs's shape changed." >&2
    echo "  This is a broken fixture, not a machine without agents." >&2
    exit 1
  fi
  # Every runnable candidate, then a ranked pick — not the first hit.
  #
  # The ranking has one rule and a reason: a `#!` script beats a native
  # binary. `fake-claude` is a bash script NAMED `claude`, so the arm it
  # already covers is "the kernel name is the agent's name". A native
  # `claude` re-covers exactly that arm. An interpreted CLI is reported by
  # the kernel as `node` / `bash` / `python3`, so it is the only shape that
  # exercises the command-line and package-path arms — the ones a stand-in
  # cannot fake. `QA_REAL_AGENT_NAME` preset in the environment overrides
  # the pick entirely.
  FOUND_SCRIPT="" FOUND_SCRIPT_NAME="" FOUND_ANY="" FOUND_ANY_NAME=""
  WANT="${QA_REAL_AGENT_NAME:-}"
  while IFS="$(printf '\t')" read -r label exe; do
    [ -n "$exe" ] || continue
    [ -z "$WANT" ] || [ "$WANT" = "$label" ] || continue
    TRIED="$TRIED $exe"
    path="$(command -v "$exe" 2>/dev/null)" || continue
    [ -n "$path" ] || continue
    # Installed is not runnable: this machine has a `codex` whose vendored
    # native binary is missing, so it prints ENOENT and exits inside a
    # second. An agent that is gone before the first probe would fail this
    # stage for a reason that is not the product's.
    if ! run_py "$HERE/probe_alive.py" "$path" 3; then
      echo "  $exe found at $path but did not stay alive; skipping it"
      continue
    fi
    if head -c2 "$path" 2>/dev/null | grep -q '#!'; then
      echo "  runnable: $label -> $path (interpreted — the interesting shape)"
      [ -n "$FOUND_SCRIPT" ] || { FOUND_SCRIPT="$path"; FOUND_SCRIPT_NAME="$label"; }
    else
      echo "  runnable: $label -> $path (native)"
      [ -n "$FOUND_ANY" ] || { FOUND_ANY="$path"; FOUND_ANY_NAME="$label"; }
    fi
  done <<< "$ROSTER"
  if [ -n "$FOUND_SCRIPT" ]; then
    QA_REAL_AGENT="$FOUND_SCRIPT"; QA_REAL_AGENT_NAME="$FOUND_SCRIPT_NAME"
  elif [ -n "$FOUND_ANY" ]; then
    QA_REAL_AGENT="$FOUND_ANY"; QA_REAL_AGENT_NAME="$FOUND_ANY_NAME"
  fi
  [ -z "$QA_REAL_AGENT" ] || echo "  picked: $QA_REAL_AGENT_NAME -> $QA_REAL_AGENT"
  export QA_REAL_AGENT_TRIED="${TRIED# }"
  if [ -z "$QA_REAL_AGENT" ]; then
    echo "  no runnable agent on PATH; the stage will SKIP and assert nothing"
  else
    # Stage a local `node_modules/.bin` so `npx <name>` resolves offline. npx
    # with nothing local would go to the network, and a fixture that needs
    # the network is a fixture that fails for the wrong reason.
    # UNDER $WORK, not $QA_ROOT: `workspace_root` is $WORK, and `pty.spawn`
    # refuses a cwd outside it — the first run of this stage staged the
    # package beside it and got "cwd … is outside every registered
    # workspace", which reads like a fixture path bug and IS one.
    NPXPKG="$WORK/npxpkg"
    if command -v npx >/dev/null 2>&1; then
      mkdir -p "$NPXPKG/node_modules/.bin"
      printf '{"name":"aleph-qa-terminal","version":"1.0.0"}\n' >"$NPXPKG/package.json"
      ln -sf "$QA_REAL_AGENT" "$NPXPKG/node_modules/.bin/$QA_REAL_AGENT_NAME"
      export QA_REAL_NPX="$(cd "$NPXPKG" && pwd -P)"
      echo "  npx package staged at $QA_REAL_NPX"
    else
      echo "  no npx on PATH; the wrapper half will SKIP"
    fi
  fi
fi

BOARD="$QA_ROOT/panel-board.json"
export QA_PANEL_BOARD="$BOARD"

say "drive: $STAGE"
RC=0
node "$HERE/drive_terminal.mjs" \
  "ws://127.0.0.1:$GATEWAY_PORT/ws" "$STAGE" "$BIN_DIR" "$WORK" "$BIN_DIR/chrome.json" || RC=$?

if [ "$STAGE" = "tui" ] && [ "$RC" = "0" ]; then
  say "drive the real aleph-tui"
  WANT_PROGRAM="$(board_field expected_program)"
  echo "  tui: $TUI_BIN   expecting the panel to show: $WANT_PROGRAM"
  run_py "$HERE/drive_tui.py" "$TUI_BIN" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" "$WANT_PROGRAM" || RC=$?
fi

if [ "$STAGE" = "panel" ] && [ "$RC" = "0" ]; then
  AGENT_S="$(board_field agent_session)"
  PLAIN_S="$(board_field plain_session)"
  WANT_AGENT="$(board_field expected_agent)"
  WANT_PROGRAM="$(board_field expected_program)"
  CTRL_PROGRAM="$(board_field control_program)"

  cat <<CHECKLIST

================================================================================
  qa/terminal panel — BROWSER checklist (chrome-devtools-mcp / claude-in-chrome)
================================================================================

  Open   http://127.0.0.1:$GATEWAY_PORT/    and switch to the Terminal panel.
  Load   qa/terminal/panel_probe.js  via evaluate_script — it installs
         \`globalThis.qaTerm\` and every item below is written against its
         RETURN VALUE, never against "the click happened".

  The board (already true on the wire — the server has been asked and answered,
  so anything the Panel shows that disagrees is the PANEL's defect):

    agent session   $AGENT_S   agent=$WANT_AGENT  program=$WANT_PROGRAM
    control session $PLAIN_S   agent=null         program=$CTRL_PROGRAM

  ------------------------------------------------------------------ 1. TABS --
  unit test: tabs.rs::title_prefers_osc_then_program_then_shell
             tabs.rs::closing_the_selected_tab_falls_to_a_neighbour

  [ ] 1a  qaTerm.tabs() has TWO entries.
  [ ] 1b  The agent session's tab title is NOT "sh".
          ⚠️ This is the whole point. Phase 1 titled every tab from the
          \`\$SHELL\` recorded at spawn; both sessions here were spawned as \`sh\`,
          so a tab reading "sh" is that defect and a green here means nothing
          if you skip 1c.
  [ ] 1c  The CONTROL tab's title is different from the agent tab's.
          (Without this, a Panel that titled everything "$WANT_AGENT" passes 1b.)
  [ ] 1d  Click the X on the SELECTED tab. qaTerm.tabs() afterwards has one
          entry and \`selected: true\` on it — not zero selected.

  ------------------------------------------------------------- 2. ROW CLICK --
  unit test: agent_panel.rs::agent_row_click_selects_the_session_and_switches_mode
             (proves the helper + greps its own source for \`on:click\`; neither
              half can see a click that reaches no handler in a real build)

  [ ] 2a  Switch to another panel. In the sidebar's agent list, click the row
          for $AGENT_S.
  [ ] 2b  qaTerm.route().terminalPanelMounted === true
  [ ] 2c  qaTerm.route().selectedTab is the AGENT tab, not whichever was
          selected before. Click the CONTROL row and re-read: the selection
          must MOVE. A handler that switched panels but ignored the session id
          passes 2b and fails this.

  ---------------------------------------------------------------- 3. PASTE --
  unit test: keymap.rs::cmd_v_and_ctrl_shift_v_are_left_to_the_browser_ctrl_v_is_0x16
             ("left to the browser" is a claim ABOUT THE BROWSER — no unit
              test has one)

  [ ] 3a  Focus the terminal. Put \`qa-paste-marker\` on the clipboard.
  [ ] 3b  Press Cmd+V (macOS) or Ctrl+Shift+V. \`qa-paste-marker\` appears on
          the screen. Read it back from the pty, not from the canvas:
            aleph tools invoke terminal --args '{"action":"read","session_id":"$AGENT_S"}'
          (\`--args\` is required; the payload is not positional.)
          or the \`terminal{read}\` RPC. Canvas pixels cannot spell.
  [ ] 3c  Press Ctrl+V. It must send 0x16 (literal-next) and NOT paste — the
          marker must NOT appear a second time. Skipping this arm makes 3b
          satisfiable by a keymap that pastes on everything.

  ---------------------------------------------------- 4. CURSOR VISIBILITY --
  unit test: session.rs::cursor_visible_false_is_stored_and_render_skips_the_cursor
             (stops at the model; render.rs::cursor_rect is what paints)

  [ ] 4a  before = qaTerm.inkCount().ink
  [ ] 4b  In the CONTROL session ($PLAIN_S) run:  printf '\\033[?25l'
          hidden = qaTerm.inkCount().ink        →  hidden < before
  [ ] 4c  Run:  printf '\\033[?25h'
          shown  = qaTerm.inkCount().ink        →  shown > hidden
          Compare the three to EACH OTHER, never to a literal: the count is a
          function of font, DPI and window size (判据 §18).
          ⚠️ Do 4b on a session with a blinking cursor at a prompt. If the
          screen is repainting (an agent's TUI), the delta is not the cursor.

================================================================================
  Ctrl-C when done. KEEP=1 keeps $QA_ROOT.
================================================================================

CHECKLIST
  echo "waiting — the board stays up until Ctrl-C"
  while kill -0 "$SERVER_PID" 2>/dev/null; do sleep 5; done
fi

NOW_STAMP="$(date -r "$BIN" '+%s')-$(wc -c <"$BIN" 2>/dev/null | tr -d ' ')"
if [ "$NOW_STAMP" != "$BIN_STAMP" ]; then
  echo
  echo "!!! HARNESS_BINARY_SWAPPED_MID_RUN"
  echo "!!!   started with $BIN_STAMP, ended with $NOW_STAMP"
  echo "!!!   Something rebuilt $BIN while this run was using it — a cargo"
  echo "!!!   build/check/clippy sharing target/, or another worktree."
  echo "!!!   Whatever this run reported is about TWO binaries. Re-run it"
  echo "!!!   alone before believing any failure above."
  RC=1
fi

say "server log tail"
LOGDIR="$ALEPH_HOME/logs"
if [ -d "$LOGDIR" ]; then
  tail -30 "$LOGDIR"/aleph-server.log* 2>/dev/null | tail -30
else
  tail -20 "$QA_ROOT/server.log"
fi

say "verdict: $STAGE rc=$RC"
exit "$RC"
