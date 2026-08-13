#!/usr/bin/env bash
# Real-machine QA for the managed (playwright-cli) browser driver.
#
#   ./qa/browser_managed/run.sh open      # launch, tab ids, snapshot, --config
#   ./qa/browser_managed/run.sh ambient   # a planted cwd cli.config.json is ignored
#   ./qa/browser_managed/run.sh headed    # headless=false must still open
#
# Same scratch-HOME discipline as qa/busy_input/run.sh. Unlike the other
# scenarios this one needs NO mock provider: it drives `tools.invoke`, which
# runs the tool without an agent turn, so nothing in the run needs a model.
#
# It does need two real things, and says so up front rather than discovering
# them at minute three:
#   * a `playwright-cli` binary — pinned via config so the run never triggers
#     the network install path;
#   * a browser it can launch, which is the entire point.
set -uo pipefail

SCENARIO="${1:-open}"
case "$SCENARIO" in open|ambient|headed) ;; *) echo "unknown scenario: $SCENARIO" >&2; exit 64 ;; esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SHARED="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-browser-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18797}"
PAGE_PORT="${PAGE_PORT:-18898}"
DEAD_MOCK_PORT="${DEAD_MOCK_PORT:-18999}"
MARKER="aleph-qa-marker-${RANDOM}${RANDOM}"

# Build BEFORE HOME is redirected — cargo's registry, git cache and rustup
# toolchain all live under the real HOME. See qa/README.md.
REAL_HOME="$HOME"

CLI="${PLAYWRIGHT_CLI:-$(command -v playwright-cli 2>/dev/null)}"
if [ -z "$CLI" ]; then
  echo "no playwright-cli on PATH; set PLAYWRIGHT_CLI=/path/to/playwright-cli" >&2
  exit 69
fi

export HOME="$QA_ROOT/home"
export ALEPH_HOME="$QA_ROOT/home/.aleph"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
UDD="$QA_ROOT/browser-profile"
PLANTED_UDD="$QA_ROOT/PLANTED-profile"
PAGEDIR="$QA_ROOT/page"
# The server's cwd — `ambient` plants a config here, every scenario runs from it.
CWD="$QA_ROOT/cwd"
mkdir -p "$PAGEDIR" "$CWD"

export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --no-deps --format-version 1 2>/dev/null \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])' 2>/dev/null)"
BIN="${TARGET_DIR:-$REPO/target}/debug/aleph-server"
SERVER_PID=""
PAGE_PID=""

say() { printf '\n=== %s ===\n' "$*"; }

cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  [ -n "$PAGE_PID" ] && kill "$PAGE_PID" 2>/dev/null
  # Close whatever the run launched — with the scratch HOME, because the CLI's
  # session store is HOME-scoped and the developer's own sessions are not ours
  # to kill.
  HOME="$QA_ROOT/home" timeout 60 "$CLI" kill-all >/dev/null 2>&1
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$PAGE_PID" ] && kill -9 "$PAGE_PID" 2>/dev/null
  if [ "$KEEP" = "1" ]; then
    echo "artifacts kept in $QA_ROOT"
  else
    rm -rf "$QA_ROOT"
  fi
}
trap cleanup EXIT

say "build ($SCENARIO)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! (cd "$REPO" && HOME="$REAL_HOME" cargo build --bin aleph-server 2>&1 | tail -5); then
    echo "build failed" >&2; exit 1
  fi
fi
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

say "serve the page fixture on 127.0.0.1:$PAGE_PORT"
cat > "$PAGEDIR/index.html" <<HTML
<!doctype html><html><head><title>Aleph browser QA</title></head>
<body><h1>$MARKER</h1><p>managed driver fixture</p></body></html>
HTML
(cd "$PAGEDIR" && python3 -m http.server "$PAGE_PORT" --bind 127.0.0.1) >"$QA_ROOT/page.log" 2>&1 &
PAGE_PID=$!
for _ in $(seq 1 30); do
  curl -sf -o /dev/null "http://127.0.0.1:$PAGE_PORT/" 2>/dev/null && break
  sleep 0.3
done

say "generate a baseline config"
timeout 25 "$BIN" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
HEADLESS=true
[ "$SCENARIO" = "headed" ] && HEADLESS=false
# Reuse the busy_input shaper for "make it inert + set the gateway port" rather
# than growing a second copy; MOCK_PORT is a dead port on purpose — this
# scenario never runs an agent turn, so the provider it writes is never dialled.
python3 "$SHARED/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$DEAD_MOCK_PORT" || exit 1
python3 "$HERE/add_browser_config.py" "$CONFIG" \
  --cli-binary "$CLI" --user-data-dir "$UDD" --headless "$HEADLESS" || exit 1

# The browser tools consult the approval policy, whose path is derived from
# $HOME (not $ALEPH_HOME — `ConfigApprovalPolicy::config_path` uses
# `dirs::home_dir()`). Allow the verbs this fixture drives; the scenario is
# about whether the browser is reachable, not about the approval gate, and an
# unanswered `ask` on a headless daemon would just time the run out.
cat > "$HOME/.aleph/approval-policy.json" <<JSON
{"defaults":{"browser_open":"allow","browser_navigate":"allow"},"allowlist":[],"blocklist":[]}
JSON

if [ "$SCENARIO" = "ambient" ]; then
  say "plant an ambient .playwright/cli.config.json in the server's cwd"
  mkdir -p "$CWD/.playwright"
  cat > "$CWD/.playwright/cli.config.json" <<JSON
{"browser":{"userDataDir":"$PLANTED_UDD"}}
JSON
  echo "planted userDataDir=$PLANTED_UDD"
fi

say "start server (cwd=$CWD)"
(cd "$CWD" && "$BIN" start) >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 90); do
  if curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null; then break; fi
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

say "drive: $SCENARIO"
RC=0
python3 "$HERE/drive_browser.py" \
  "ws://127.0.0.1:$GATEWAY_PORT/ws" "$SCENARIO" \
  --page-url "http://127.0.0.1:$PAGE_PORT/" \
  --marker "$MARKER" \
  --home "$QA_ROOT/home" \
  --cli "$CLI" \
  --expect-user-data-dir "$UDD" \
  --planted-user-data-dir "$PLANTED_UDD" \
  --cwd "$CWD" \
  --output-dir-root "$ALEPH_HOME/data/browser/cli-output" || RC=$?

say "cli sessions at the end"
HOME="$QA_ROOT/home" timeout 60 "$CLI" list 2>&1 | head -20

say "server log tail"
LOGDIR="$ALEPH_HOME/logs"
if [ -d "$LOGDIR" ]; then
  grep -iE "playwright|browser|session opened" "$LOGDIR"/aleph-server.log* 2>/dev/null | tail -20
else
  tail -20 "$QA_ROOT/server.log"
fi

say "verdict: rc=$RC"
exit "$RC"
