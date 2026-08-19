#!/usr/bin/env bash
# Real-machine QA for the plugin-ecosystem round.
#
#   ./qa/plugins/run.sh manifest   # component-field union, marketplace source
#                                  # union, plugin-root expansion, durable config
#   ./qa/plugins/run.sh scaffold   # `aleph plugin init` output really installs
#
# The round this covers shipped with unit and source-level guards only. Two of
# its headline fixes are `serde` all-or-nothing bugs, and those have a specific
# property that makes in-process tests weak evidence: the failure is not a bad
# field, it is a *rejected document*, and the registry's response to a rejected
# document is a row that looks like a plugin which simply ships nothing. Only a
# daemon holding a real registry can tell those apart.
#
# Everything lands in a scratch HOME/ALEPH_HOME under $QA_ROOT, so this never
# touches the developer's ~/.aleph (two processes on one vault is a documented
# way to lose vault data -- PROCESS_MANAGEMENT.md).
set -uo pipefail

SCENARIO="${1:-manifest}"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
# Deliberately short, and deliberately NOT under `$TMPDIR` like the sibling
# fixtures. The hook inventory elides action labels at 80 characters -- a
# documented "what is wired up" listing, not a config dump -- and macOS spells
# `$TMPDIR` as a 48-character path. Under it, the elision lands mid-path and
# cuts off the plugin id, which is the one segment that distinguishes
# "expanded to this plugin's root" from "expanded to something". A short root
# keeps the whole expanded command inside the budget so the assertion can be
# exact. Don't "tidy" this back to $TMPDIR without re-reading phase C.
QA_ROOT="${QA_ROOT:-$(mktemp -d "/tmp/aleph-qa-plg-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18801}"
MOCK_PORT="${MOCK_PORT:-18802}"   # nothing listens; the config must merely not name a real provider

# Build BEFORE HOME is redirected: cargo's registry, git cache and rustup
# toolchain all live under the real HOME, and a build launched with the scratch
# one silently degrades into a full network fetch that then times out.
. "$HERE/../lib/scratch_home.sh"
# Redirects HOME/ALEPH_HOME into the scratch root AND pins RUSTUP_HOME/
# CARGO_HOME at the real ones -- the redirect and the pin are inseparable on
# purpose; see that file for the 1.3 GB-per-run leak it closes.
qa_redirect_home "$QA_ROOT"
export REAL_HOME
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
INSTALLED="$ALEPH_HOME/plugins/installed"
MARKETPLACES="$QA_ROOT/marketplaces"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

SERVER_PID=""
say() { printf '\n=== %s ===\n' "$*"; }
stop_server() {
  [ -n "$SERVER_PID" ] || return 0
  kill "$SERVER_PID" 2>/dev/null
  # The singleton lock is released on exit, not on SIGTERM delivery: a restart
  # that races it comes up as a second instance on one vault, which is the
  # documented way to lose vault data. Wait for the process to be gone.
  for _ in $(seq 1 30); do kill -0 "$SERVER_PID" 2>/dev/null || break; sleep 0.5; done
  kill -9 "$SERVER_PID" 2>/dev/null
  wait "$SERVER_PID" 2>/dev/null
  SERVER_PID=""
}
cleanup() {
  stop_server
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  # Two packages, so two invocations: `-p` is positional-ish here -- a single
  # `cargo build -p aleph-cli --bin aleph-server --bin aleph` resolves BOTH
  # `--bin` flags against `aleph-cli` and fails on the server.
  if ! (cd "$REPO" && HOME="$REAL_HOME" cargo build -p alephcore --bin aleph-server 2>&1 | tail -5); then
    echo "build failed" >&2; exit 1
  fi
  if ! (cd "$REPO" && HOME="$REAL_HOME" cargo build -p aleph-cli --bin aleph 2>&1 | tail -5); then
    echo "cli build failed" >&2; exit 1
  fi
fi
# Ask cargo where its target dir really is: `.cargo/config.toml` pins a shared
# absolute one, so a hardcoded `$REPO/target` is wrong from any git worktree.
TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')"
BIN="$TARGET_DIR/debug/aleph-server"
CLI="$TARGET_DIR/debug/aleph"
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

say "generate a baseline config"
timeout 25 "$BIN" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
python3 "$BUSY/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1

start_server() {
  # stdout is not a TTY here, so tracing goes to $ALEPH_HOME/logs/ -- the
  # redirect below catches only the startup banner. "No output" is not
  # "nothing happened".
  "$BIN" start >>"$QA_ROOT/server.log" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 90); do
    curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && return 0
    kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; return 1; }
    sleep 1
  done
  echo "gateway never came up"; tail -40 "$QA_ROOT/server.log"; return 1
}

RC=0

case "$SCENARIO" in
manifest)
  say "plant plugin trees"
  python3 "$HERE/plant_plugins.py" "$INSTALLED" "$MARKETPLACES" || exit 1
  find "$INSTALLED" -maxdepth 2 | sed "s|$QA_ROOT|\$QA_ROOT|" | head -20

  say "start server"
  start_server || exit 1
  echo "gateway up on $GATEWAY_PORT"

  say "drive (pre-restart)"
  python3 "$HERE/drive_plugins.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" "$ALEPH_HOME" "$MARKETPLACES/qa-market" pre || RC=$?

  say "restart server"
  stop_server
  start_server || exit 1

  say "drive (post-restart)"
  python3 "$HERE/drive_plugins.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" "$ALEPH_HOME" "$MARKETPLACES/qa-market" post || RC=$?
  ;;

scaffold)
  # The claim: `aleph plugin init --type <runtime>` writes a manifest the
  # SERVER can load. Round 1 found the scaffolder and the parser were two
  # authors -- `--type nodejs` wrote `kind = "nodejs"`, which `PluginKind`
  # rejects, so the documented first example produced a plugin that could
  # never load. The test that was supposed to cover it asserted the literal
  # the scaffolder had just written, so it passed throughout.
  #
  # Driving the real CLI and then the real server is the only way to check
  # that those two agree; anything in-process re-reads one author's opinion.
  say "scaffold one plugin per runtime, install, and load"
  [ -x "$CLI" ] || { echo "no aleph CLI at $CLI" >&2; exit 1; }
  RUNTIMES="$(python3 - "$REPO" <<'PY'
import re, sys, pathlib
src = pathlib.Path(sys.argv[1], "shared/protocol/src/plugins.rs").read_text()
m = re.search(r'PLUGIN_RUNTIMES:\s*\[&str;\s*\d+\]\s*=\s*\[(.*?)\]', src, re.S)
print(" ".join(re.findall(r'"([^"]+)"', m.group(1))) if m else "")
PY
)"
  # Derived from the shared vocabulary, not listed here: a fourth runtime that
  # the scaffolder learns and the loader does not must make this fail, and a
  # hand-written list here would quietly keep passing.
  [ -n "$RUNTIMES" ] || { echo "could not read PLUGIN_RUNTIMES" >&2; exit 1; }
  echo "runtimes: $RUNTIMES"
  mkdir -p "$INSTALLED"
  for rt in $RUNTIMES; do
    ( cd "$QA_ROOT" && "$CLI" plugin init "qa-scaffold-$rt" --type "$rt" >"$QA_ROOT/init-$rt.log" 2>&1 ) \
      || { echo "  [FAIL] plugin init --type $rt exited nonzero"; cat "$QA_ROOT/init-$rt.log"; RC=1; continue; }
    SRC="$(find "$QA_ROOT" -maxdepth 2 -type d -name "qa-scaffold-$rt" ! -path "$INSTALLED/*" | head -1)"
    [ -n "$SRC" ] || { echo "  [FAIL] init --type $rt produced no directory"; RC=1; continue; }
    ( cd "$SRC" && "$CLI" plugin validate . >"$QA_ROOT/validate-$rt.log" 2>&1 ) \
      || { echo "  [FAIL] plugin validate rejected its own scaffold ($rt)"; cat "$QA_ROOT/validate-$rt.log"; RC=1; }
    cp -R "$SRC" "$INSTALLED/qa-scaffold-$rt"
  done

  say "start server"
  start_server || exit 1

  say "does the SERVER load what the CLI wrote?"
  python3 "$HERE/drive_scaffold.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" "$RUNTIMES" || RC=$?
  ;;

*)
  echo "unknown scenario '$SCENARIO' (manifest | scaffold)" >&2; exit 2;;
esac

say "server warnings about plugins"
grep -i "plugin" "$ALEPH_HOME"/logs/*.log 2>/dev/null | grep -iE "warn|error" | head -20

say "verdict: rc=$RC"
exit "$RC"
