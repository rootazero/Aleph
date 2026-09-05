#!/usr/bin/env bash
# Real-machine QA for the managed (playwright-cli) browser driver.
#
#   ./qa/browser_managed/run.sh open      # launch, tab ids, snapshot, --config
#   ./qa/browser_managed/run.sh ambient   # a planted cwd cli.config.json is ignored
#   ./qa/browser_managed/run.sh headed    # headless=false must still open
#   ./qa/browser_managed/run.sh tools     # every remaining browser verb, asserted by EFFECT
#   ./qa/browser_managed/run.sh frames    # a genuinely cross-origin iframe (2nd port)
#   ./qa/browser_managed/run.sh reap      # the idle reaper really closes a session (~2 min)
#   ./qa/browser_managed/run.sh pdf       # pdf_generate's browser engine
#   ./qa/browser_managed/run.sh existing  # the OTHER driver (Chrome DevTools MCP)
#   ./qa/browser_managed/run.sh exec-offload # browser_exec's spill, inside a real turn
#   ./qa/browser_managed/run.sh attach    # Aleph starts Chrome; playwright-cli joins over CDP
#
# Same scratch-HOME discipline as qa/busy_input/run.sh. Every scenario but
# `exec-offload` needs NO mock provider: they drive `tools.invoke`, which runs
# the tool without an agent turn, so nothing in the run needs a model.
# `exec-offload` is the exception on purpose — the branch it tests is reachable
# only from inside a turn, because the spill is keyed by a tool call id the
# harness Act phase mints and `tools.invoke` has none.
#
# It does need two real things, and says so up front rather than discovering
# them at minute three:
#   * a `playwright-cli` binary — pinned via config so the run never triggers
#     the network install path;
#   * a browser it can launch, which is the entire point.
set -uo pipefail

SCENARIO="${1:-open}"
case "$SCENARIO" in
  open|ambient|headed|tools|frames|reap|pdf|existing|exec-offload|attach) ;;
  *) echo "unknown scenario: $SCENARIO" >&2; exit 64 ;;
esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SHARED="$HERE/../busy_input"
QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-browser-XXXXXX")}"
KEEP="${KEEP:-0}"

GATEWAY_PORT="${GATEWAY_PORT:-18797}"
PAGE_PORT="${PAGE_PORT:-18898}"
# A SECOND origin. Same host, different port — which is a different origin under
# the same-origin policy, and therefore a real out-of-process iframe. That is
# what makes the `frames` scenario about OOPIF rather than about a same-origin
# frame that would satisfy every claim for the wrong reason.
PAGE2_PORT="${PAGE2_PORT:-18899}"
DEAD_MOCK_PORT="${DEAD_MOCK_PORT:-18999}"
# `exec-offload` is the only scenario that dials the provider, so it is the only
# one that gets a live port. The others keep the dead one, which is what makes
# "no model was involved" a property of the fixture rather than a hope.
LIVE_MOCK_PORT="${LIVE_MOCK_PORT:-18995}"
MOCK_PID=""
MARKER="aleph-qa-marker-${RANDOM}${RANDOM}"
CONSOLE_MARKER="aleph-qa-console-${RANDOM}"
LATE_MARKER="aleph-qa-late-${RANDOM}"
CHILD_MARKER="aleph-qa-child-${RANDOM}"
CONTROL_PROFILE="control"
CONTROL_UDD="$QA_ROOT/control-profile"
CONTROL_MAX_TABS=2
# `idle_timeout_secs` is per-profile config, so the reaper is observable inside
# a QA run instead of at the 30-minute default. The sweep interval is NOT
# configurable (`spawn_idle_reaper(60)` at the registry construction site), so
# the wait is bounded below by one sweep, not by the timeout.
REAP_IDLE_SECS=5
REAP_WAIT_SECS="${REAP_WAIT_SECS:-150}"

# Build BEFORE HOME is redirected — cargo's registry, git cache and rustup
# toolchain all live under the real HOME. See qa/README.md. (`REAL_HOME` is
# captured by `qa_redirect_home` below, at the point of the redirect itself.)

CLI="${PLAYWRIGHT_CLI:-$(command -v playwright-cli 2>/dev/null)}"
if [ -z "$CLI" ]; then
  echo "no playwright-cli on PATH; set PLAYWRIGHT_CLI=/path/to/playwright-cli" >&2
  exit 69
fi

. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
# Redirects HOME/ALEPH_HOME into the scratch root AND pins RUSTUP_HOME/
# CARGO_HOME at the real ones — the redirect and the pin are inseparable
# on purpose; see that file for the 1.3 GB-per-run leak it closes.
qa_redirect_home "$QA_ROOT"
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
PAGE2_PID=""
# A file the `existing` scenario uploads from OUTSIDE the OS temp dir.
# QA_ROOT lives UNDER $TMPDIR, and chrome-devtools-mcp's path guard — when it is
# armed at all — allowlists exactly tmpdir plus the client's declared roots. An
# upload out of QA_ROOT would therefore pass whether the guard is inert or not,
# which is the shape of a claim that cannot fail. `target/` is gitignored and
# lives with the checkout, i.e. not under tmpdir; the fixture asserts that
# rather than assuming it.
OUTSIDE_TMP_DIR=""

say() { printf '\n=== %s ===\n' "$*"; }

cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  [ -n "$MOCK_PID" ] && kill -9 "$MOCK_PID" 2>/dev/null
  [ -n "$PAGE_PID" ] && kill "$PAGE_PID" 2>/dev/null
  [ -n "$PAGE2_PID" ] && kill "$PAGE2_PID" 2>/dev/null
  # Close whatever the run launched — with the scratch HOME, because the CLI's
  # session store is HOME-scoped and the developer's own sessions are not ours
  # to kill.
  HOME="$QA_ROOT/home" timeout 60 "$CLI" kill-all >/dev/null 2>&1
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  [ -n "$PAGE_PID" ] && kill -9 "$PAGE_PID" 2>/dev/null
  [ -n "$PAGE2_PID" ] && kill -9 "$PAGE2_PID" 2>/dev/null
  [ -n "$OUTSIDE_TMP_DIR" ] && rm -rf "$OUTSIDE_TMP_DIR"
  if [ "$KEEP" = "1" ]; then
    echo "artifacts kept in $QA_ROOT"
  else
    rm -rf "$QA_ROOT"
  fi
}
trap cleanup EXIT

say "build ($SCENARIO)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  if ! qa_build --bin aleph-server; then
    echo "build failed" >&2; exit 1
  fi
fi
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

say "serve the page fixture on 127.0.0.1:$PAGE_PORT"
PAGE2DIR="$QA_ROOT/page2"
mkdir -p "$PAGE2DIR"
PARENT_ORIGIN="http://127.0.0.1:$PAGE_PORT"
CHILD_ORIGIN="http://127.0.0.1:$PAGE2_PORT"

cat > "$PAGEDIR/index.html" <<HTML
<!doctype html><html><head><title>Aleph browser QA</title></head>
<body><h1>$MARKER</h1><p>managed driver fixture</p></body></html>
HTML
cat > "$PAGEDIR/second.html" <<HTML
<!doctype html><html><head><title>Aleph browser QA second</title></head>
<body><h1>$MARKER second page</h1></body></html>
HTML
# The interactive fixture and the frame pages are checked-in files with marker
# placeholders, not heredocs: they carry event handlers whose exact text is the
# oracle for "did this verb do anything", and a heredoc would put that behind
# two layers of shell quoting.
sed -e "s/__MARKER__/$MARKER/g" \
    -e "s/__CONSOLE_MARKER__/$CONSOLE_MARKER/g" \
    -e "s/__LATE_MARKER__/$LATE_MARKER/g" \
    "$HERE/pages/tools.html" > "$PAGEDIR/tools.html"
sed -e "s/__MARKER__/$MARKER/g" \
    -e "s|__CHILD_URL__|$CHILD_ORIGIN/frames_child.html|g" \
    "$HERE/pages/frames_parent.html" > "$PAGEDIR/frames_parent.html"
sed -e "s/__CHILD_MARKER__/$CHILD_MARKER/g" \
    "$HERE/pages/frames_child.html" > "$PAGE2DIR/frames_child.html"
cp "$HERE/pages/net-probe.json" "$PAGEDIR/net-probe.json"

(cd "$PAGEDIR" && python3 -m http.server "$PAGE_PORT" --bind 127.0.0.1) >"$QA_ROOT/page.log" 2>&1 &
PAGE_PID=$!
(cd "$PAGE2DIR" && python3 -m http.server "$PAGE2_PORT" --bind 127.0.0.1) >"$QA_ROOT/page2.log" 2>&1 &
PAGE2_PID=$!
for _ in $(seq 1 30); do
  curl -sf -o /dev/null "http://127.0.0.1:$PAGE_PORT/" 2>/dev/null && break
  sleep 0.3
done
for _ in $(seq 1 30); do
  curl -sf -o /dev/null "$CHILD_ORIGIN/frames_child.html" 2>/dev/null && break
  sleep 0.3
done

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
HEADLESS=true
[ "$SCENARIO" = "headed" ] && HEADLESS=false
# Reuse the busy_input shaper for "make it inert + set the gateway port" rather
# than growing a second copy; MOCK_PORT is a dead port on purpose — this
# scenario never runs an agent turn, so the provider it writes is never dialled.
PROVIDER_PORT="$DEAD_MOCK_PORT"
[ "$SCENARIO" = "exec-offload" ] && PROVIDER_PORT="$LIVE_MOCK_PORT"
python3 "$SHARED/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$PROVIDER_PORT" || exit 1
BROWSER_CFG_ARGS=(--cli-binary "$CLI" --user-data-dir "$UDD" --headless "$HEADLESS")
if [ "$SCENARIO" = "existing" ]; then
  # Pin the MCP server to the newest copy already in the developer's npx cache.
  # The default (`npx -y chrome-devtools-mcp@latest`) resolves against $HOME,
  # which this run has redirected to an empty scratch dir — so the default
  # would fetch the package from the network in the middle of the scenario, and
  # a QA verdict that depends on the network is not a verdict.
  CDP_MCP="$(ls -d "$REAL_HOME"/.npm/_npx/*/node_modules/chrome-devtools-mcp 2>/dev/null \
    | while read -r d; do
        v=$(python3 -c "import json;print(json.load(open('$d/package.json'))['version'])" 2>/dev/null)
        [ -n "$v" ] && printf '%s\t%s\n' "$v" "$d"
      done | sort -V | tail -1 | cut -f2)"
  CDP_BIN="$CDP_MCP/build/src/bin/chrome-devtools-mcp.js"
  if [ -z "$CDP_MCP" ] || [ ! -f "$CDP_BIN" ]; then
    echo "no cached chrome-devtools-mcp under $REAL_HOME/.npm/_npx;" >&2
    echo "run 'npx -y chrome-devtools-mcp@latest --help' once, then retry" >&2
    exit 69
  fi
  echo "chrome-devtools-mcp: $CDP_MCP"
  # `--chrome-mcp-arg=<v>` rather than a separate token: argparse reads a bare
  # `--isolated` as one of its own flags.
  #
  # `--isolated` is not a nicety. The production default is `--autoConnect`,
  # which attaches to the *developer's own running Chrome* — a QA run must not
  # drive the browser the person is using. A temp profile also makes the run
  # repeatable.
  BROWSER_CFG_ARGS+=(
    --existing-session-profile "existing"
    --chrome-mcp-command "$(command -v node)"
    "--chrome-mcp-arg=$CDP_BIN"
    "--chrome-mcp-arg=--isolated"
    "--chrome-mcp-arg=--headless"
    "--chrome-mcp-arg=--experimentalStructuredContent"
    # Mirrors the shipped default (`default_chrome_mcp_args`). Without it the
    # server confines every filePath argument to the OS temp dir, which is what
    # made `browser_upload` fail for a file in the checkout. This run pins the
    # binary for hermeticity, so the args have to be restated here — and a QA
    # that restated them WITHOUT this one would be testing a configuration
    # nobody ships.
    "--chrome-mcp-arg=--allow-unrestricted-paths"
  )
fi
if [ "$SCENARIO" = "reap" ]; then
  # The default profile becomes the reap candidate; a second profile with a
  # far-future timeout is the control that must survive the SAME sweep, and it
  # carries the tab cap so the LRU half needs no extra wait.
  BROWSER_CFG_ARGS+=(
    --idle-timeout-secs "$REAP_IDLE_SECS"
    --tab-idle-timeout-secs 99999
    --control-profile "$CONTROL_PROFILE"
    --control-user-data-dir "$CONTROL_UDD"
    --control-max-tabs "$CONTROL_MAX_TABS"
  )
fi
if [ "$SCENARIO" = "attach" ]; then
  # Pin the browser so the run does not depend on which browsers this
  # machine happens to have, and so the RED control has one thing to break.
  # `find_chromium`'s own first macOS path; on Linux/Windows set
  # ALEPH_QA_CHROME to override.
  CHROME_BIN="${ALEPH_QA_CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
  # The precheck is skippable BY A DOCUMENTED FLAG, not by editing this
  # file. The RED control needs to reach the fail-closed message with a
  # deliberately broken pin, and a control that only exists while a file is
  # locally modified is not repeatable and will not exist next round.
  if [ ! -x "$CHROME_BIN" ] && [ -z "${ALEPH_QA_ALLOW_MISSING_CHROME:-}" ]; then
    echo "no browser at $CHROME_BIN; set ALEPH_QA_CHROME, or set" >&2
    echo "ALEPH_QA_ALLOW_MISSING_CHROME=1 to drive the fail-closed path on purpose" >&2
    exit 69
  fi
  BROWSER_CFG_ARGS+=(--runtime-binary-path "$CHROME_BIN" --prefer-system-browser false)
  echo "chromium pinned: $CHROME_BIN"
fi
python3 "$HERE/add_browser_config.py" "$CONFIG" "${BROWSER_CFG_ARGS[@]}" || exit 1

# The browser tools consult the approval policy, whose path is derived from
# $HOME (not $ALEPH_HOME — `ConfigApprovalPolicy::config_path` uses
# `dirs::home_dir()`). Allow the verbs this fixture drives; the scenario is
# about whether the browser is reachable, not about the approval gate, and an
# unanswered `ask` on a headless daemon would just time the run out.
# The key names are the snake_case `ActionType` variants (`approval/types.rs`),
# not tool names — `browser_cookies` writes are `browser_cookies_write`, and
# `browser_emulate` / `browser_session` are `browser_identity_override` /
# `browser_session_state`. A key that names no variant is silently ignored,
# which would leave the verb at its built-in `Ask` and hang a headless run.
cat > "$HOME/.aleph/approval-policy.json" <<'JSON'
{"defaults":{
  "browser_open":"allow","browser_navigate":"allow","browser_click":"allow",
  "browser_type":"allow","browser_fill":"allow","browser_evaluate":"allow",
  "browser_select":"allow","browser_dialog":"allow","browser_press_key":"allow",
  "browser_scroll":"allow","browser_hover":"allow","browser_drag":"allow",
  "browser_upload":"allow","browser_cookies_write":"allow",
  "browser_identity_override":"allow","browser_session_state":"allow"
},"allowlist":[],"blocklist":[]}
JSON

if [ "$SCENARIO" = "ambient" ]; then
  say "plant an ambient .playwright/cli.config.json in the server's cwd"
  mkdir -p "$CWD/.playwright"
  cat > "$CWD/.playwright/cli.config.json" <<JSON
{"browser":{"userDataDir":"$PLANTED_UDD"}}
JSON
  echo "planted userDataDir=$PLANTED_UDD"
fi

if [ "$SCENARIO" = "exec-offload" ]; then
  say "start mock provider (one browser_exec call, then end)"
  # `max_chars` is the point: 1000 is the floor `browser_snapshot` clamps to, and
  # the fixture page's tree is comfortably larger, so the step is guaranteed to
  # be cut — which is the precondition for the spill this scenario is about.
  cat > "$QA_ROOT/tool-spec.json" <<JSON
{"name": "browser_exec",
 "input": {"profile": "default",
           "actions": [{"action": "snapshot", "max_chars": 1000}]}}
JSON
  REQUEST_LOG="$QA_ROOT/mock-requests.jsonl"
  # plan `quick` = one tool turn, then end. A longer plan would call the tool
  # again and again for no extra evidence.
  python3 "$SHARED/mock_anthropic.py" "$LIVE_MOCK_PORT" /etc/hostname quick \
    "$QA_ROOT/tool-spec.json" "$REQUEST_LOG" >"$QA_ROOT/mock.log" 2>&1 &
  MOCK_PID=$!
  sleep 1
fi

say "start server (cwd=$CWD)"
if [ "$SCENARIO" = "pdf" ]; then
  # Take `playwright-cli` OFF the server's PATH while leaving `binary_path`
  # pinned. Without this the scenario cannot tell "the engine honored the
  # operator's configuration" from "it happened to find a binary on PATH" — the
  # pinned path IS the one `which` finds, so every claim passes either way.
  #
  # `node` stays reachable on purpose: `playwright-cli` is a node script, and a
  # PATH without `node` turns every invocation into `env: node: not found`,
  # which would look like the very failure being tested.
  NODEBIN="$QA_ROOT/nodebin"
  mkdir -p "$NODEBIN"
  ln -sf "$(command -v node)" "$NODEBIN/node"
  echo "server PATH excludes $(dirname "$CLI"); node symlinked into $NODEBIN"
  (cd "$CWD" && PATH="$NODEBIN:/usr/bin:/bin" "$BIN" start) >"$QA_ROOT/server.log" 2>&1 &
else
  (cd "$CWD" && "$BIN" start) >"$QA_ROOT/server.log" 2>&1 &
fi
SERVER_PID=$!

for _ in $(seq 1 90); do
  if curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null; then break; fi
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

say "drive: $SCENARIO"
RC=0
OUTDIR="$QA_ROOT/out"
mkdir -p "$OUTDIR"
UPLOAD_FILE="$QA_ROOT/upload-me.txt"
echo "$MARKER upload payload" > "$UPLOAD_FILE"
OUTSIDE_TMP_DIR="${TARGET_DIR:-$REPO/target}/qa-browser-outside-tmp"
mkdir -p "$OUTSIDE_TMP_DIR"
OUTSIDE_TMP_FILE="$OUTSIDE_TMP_DIR/upload-me-outside-tmp.txt"
echo "$MARKER upload payload (outside the OS temp dir)" > "$OUTSIDE_TMP_FILE"

case "$SCENARIO" in
  exec-offload)
    python3 "$HERE/drive_exec_offload.py" \
      "ws://127.0.0.1:$GATEWAY_PORT/ws" \
      --page-url "http://127.0.0.1:$PAGE_PORT/tools.html" \
      --request-log "$QA_ROOT/mock-requests.jsonl" \
      --marker "$MARKER" || RC=$?
    ;;
  open|ambient|headed)
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
    ;;
  attach)
    python3 "$HERE/drive_attach.py" \
      "ws://127.0.0.1:$GATEWAY_PORT/ws" \
      --page-url "http://127.0.0.1:$PAGE_PORT/" \
      --marker "$MARKER" \
      --home "$QA_ROOT/home" \
      --cli "$CLI" \
      --expect-user-data-dir "$UDD" \
      --server-pid "$SERVER_PID" || RC=$?
    # The driver SIGTERMs the server as its LAST claim, reached only if
    # everything before it passed. Clearing this unconditionally would orphan
    # the server (and the Chrome it launched) whenever the driver dies or
    # raises before reaching that step — which really happened once during
    # this task, from an unrelated bug earlier in the driver. So only treat
    # the server as already-gone when the driver actually exited clean.
    if [ "$RC" -eq 0 ]; then
      SERVER_PID=""
    fi
    ;;
  *)
    # Each scenario gets the page it is about; `reap` and `pdf` only need
    # *some* page, so they reuse the simple one.
    case "$SCENARIO" in
      tools|existing) DRIVE_URL="http://127.0.0.1:$PAGE_PORT/tools.html" ;;
      frames) DRIVE_URL="http://127.0.0.1:$PAGE_PORT/frames_parent.html" ;;
      *)      DRIVE_URL="http://127.0.0.1:$PAGE_PORT/" ;;
    esac
    python3 "$HERE/drive_tools.py" \
      "ws://127.0.0.1:$GATEWAY_PORT/ws" "$SCENARIO" \
      --page-url "$DRIVE_URL" \
      --second-url "http://127.0.0.1:$PAGE_PORT/second.html" \
      --marker "$MARKER" \
      --console-marker "$CONSOLE_MARKER" \
      --late-marker "$LATE_MARKER" \
      --child-marker "$CHILD_MARKER" \
      --parent-origin "$PARENT_ORIGIN" \
      --child-origin "$CHILD_ORIGIN" \
      --home "$QA_ROOT/home" \
      --cli "$CLI" \
      --out-dir "$OUTDIR" \
      --upload-file "$UPLOAD_FILE" \
      --user-data-dir "$UDD" \
      --control-profile "$CONTROL_PROFILE" \
      --control-user-data-dir "$CONTROL_UDD" \
      --control-max-tabs "$CONTROL_MAX_TABS" \
      --existing-profile "existing" \
      --outside-tmp-file "$OUTSIDE_TMP_FILE" \
      --idle-secs "$REAP_IDLE_SECS" \
      --wait-secs "$REAP_WAIT_SECS" || RC=$?
    ;;
esac

if [ "$SCENARIO" = "exec-offload" ]; then
  say "mock provider log"
  tail -20 "$QA_ROOT/mock.log"
fi

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
