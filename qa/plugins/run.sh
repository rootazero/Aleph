#!/usr/bin/env bash
# Real-machine QA for the plugin-ecosystem round.
#
#   ./qa/plugins/run.sh manifest   # component-field union, marketplace source
#                                  # union, plugin-root expansion, durable config
#   ./qa/plugins/run.sh scaffold   # `aleph plugin init` output really installs
#   ./qa/plugins/run.sh browse     # marketplace contents are listable, and a
#                                  # name found that way actually installs
#   ./qa/plugins/run.sh marketplaces # the registration surface: list / add /
#                                  # remove, and the removable bit the Panel
#                                  # draws its button from
#   ./qa/plugins/run.sh scope      # MCP plugin enable → disable → enable on a real
#                                  # daemon; `tools.catalog` changes each time; no
#                                  # mount ran before its handle was installed
#   ./qa/plugins/run.sh panel      # BOOTS AND WAITS: the same surface through
#                                  # the browser, plus the source classifier
#   ./qa/plugins/run.sh visibility # a project's .aleph/plugins/<p> plugin (skill,
#                                  # agent, MCP tool) reaches the model only for
#                                  # a run bound to that project
#   ./qa/plugins/run.sh command    # `/cmd args` puts the command's rendered body
#                                  # in front of the model; the persisted turn
#                                  # stays the raw `/cmd` text
#   ./qa/plugins/run.sh exit2      # a `>&2; exit 2` PreToolUse hook, approved the
#                                  # operator's way, blocks file_read and its
#                                  # stderr is the tool result the model reads
#   ./qa/plugins/run.sh subagent   # a command's `allowed-tools` restriction also
#                                  # bounds the `subagent` child it delegates to
#   ./qa/plugins/run.sh cc-cache  # a Claude Code install under ~/.claude/plugins:
#                                  # listed, disabled, the model may not enable it,
#                                  # the operator can; its .mcp.json server mounts;
#                                  # durable; nothing under ~/.claude is written
#
# `marketplaces` drives WebSocket-RPC; `panel` is the DOM half of the same
# screen and is deliberately separate. The RPC fixture cannot see anything the
# renderer decides -- whether the built-in row draws a Remove button the server
# would refuse, whether a refusal is shown or silently rendered as "none
# registered", whether an attribute a stylesheet keys off is actually set. Each
# of those has been a real defect in this repo on a first browser run.
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

# Every stage is python-driven from the preamble on (`cargo metadata |
# python3 -c …` resolves the target dir, `patch_config.py` makes the config
# inert) and the `scope` stage's mock MCP server is python too. A host without
# it is UNRUN (exit 2), never PASS — and never a `no binary at …` exit 1 that
# reads like a build problem. This is the FIRST line that does anything:
# `command -v` is a builtin, so it answers with an empty PATH, before
# `dirname` / `mktemp` below would misbehave in silence.
command -v python3 >/dev/null || { echo "UNRUN: python3 not on PATH (every stage's config patcher and driver is python)"; exit 2; }

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
BUSY="$HERE/../busy_input"
# Deliberately short, and deliberately NOT under `$TMPDIR` like the sibling
# fixtures. The hook inventory elides action labels at 80 characters -- a
# documented "what is wired up" listing, not a config dump -- and macOS spells
# `$TMPDIR` as a 48-character path. Under it, the elision lands mid-path and
# cuts off the plugin id. Phase C was written when a plugin hook's command was
# stored with the install path expanded into it (P4.16 removed that splice: it
# now asserts the command comes back as written and the path is absent), and
# the seat-rules still ask for a short QA_ROOT; the paths every stage prints
# stay readable. Don't "tidy" this back to $TMPDIR without re-reading phase C.
QA_ROOT="${QA_ROOT:-$(mktemp -d "/tmp/aleph-qa-plg-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18801}"
# Only `visibility` runs a mock provider here; for every other stage nothing
# listens, and the config must merely not name a real provider.
MOCK_PORT="${MOCK_PORT:-18802}"
# Where `start_server` launches the daemon. It matters to exactly one rule: a
# run bound to no project takes the daemon's CWD as its project root
# (`VisibilityCtx::from_project_root`), so the CWD decides which project
# plugins a project-less run sees. `visibility` points it at $QA_ROOT.
SERVER_CWD="${SERVER_CWD:-$REPO}"

# Build BEFORE HOME is redirected: cargo's registry, git cache and rustup
# toolchain all live under the real HOME, and a build launched with the scratch
# one silently degrades into a full network fetch that then times out.
. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
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
MOCK_PID=""
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
  [ -n "$MOCK_PID" ] && { kill "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null; }
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "build"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  # Two packages, so two invocations: `-p` is positional-ish here -- a single
  # `cargo build -p aleph-cli --bin aleph-server --bin aleph` resolves BOTH
  # `--bin` flags against `aleph-cli` and fails on the server.
  if ! qa_build -p alephcore --bin aleph-server; then
    echo "build failed" >&2; exit 1
  fi
  if ! qa_build -p aleph-cli --bin aleph; then
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
python3 "$BUSY/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1

start_server() {
  # stdout is not a TTY here, so tracing goes to $ALEPH_HOME/logs/ -- the
  # redirect below catches only the startup banner. "No output" is not
  # "nothing happened". The subshell `exec`s, so `$!` is the server itself
  # (a `( cd …; start_server )` around the caller would lose SERVER_PID).
  ( cd "$SERVER_CWD" && exec "$BIN" start ) >>"$QA_ROOT/server.log" 2>&1 &
  SERVER_PID=$!
  for _ in $(seq 1 90); do
    curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && return 0
    kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; return 1; }
    sleep 1
  done
  echo "gateway never came up"; tail -40 "$QA_ROOT/server.log"; return 1
}

# The mock Anthropic provider (`busy_input/mock_anthropic.py`), recording every
# request it answers to $4 — the only witness to what the MODEL was handed.
# Args: <probe_path> <plan> <tool_spec or ""> <request_log>. Replaces any mock
# this run already started: its turn counter is global, so a second phase that
# needs a fresh plan needs a fresh process.
start_mock() {
  if [ -n "$MOCK_PID" ]; then kill "$MOCK_PID" 2>/dev/null; wait "$MOCK_PID" 2>/dev/null; MOCK_PID=""; fi
  python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" "$1" "$2" "$3" "$4" >>"$QA_ROOT/mock.log" 2>&1 &
  MOCK_PID=$!
  for _ in $(seq 1 20); do
    curl -sf -o /dev/null "http://127.0.0.1:$MOCK_PORT/v1/models" 2>/dev/null && return 0
    sleep 0.5
  done
  echo "mock provider never came up"; tail -20 "$QA_ROOT/mock.log"; return 1
}

# A `Simulated` daemon never calls the mock, so every assertion about what the
# model received would fail for a reason that has nothing to do with plugins.
# Name the cause first (the `visibility` arm does the same inline).
assert_real_mode() {
  local line
  line="$(grep "Mode:" "$QA_ROOT/server.log" | tail -1)"
  echo "  ${line:-(no Mode: line in the server log)}"
  case "$line" in
    *"Real AgentLoop"*) echo "  [PASS] the daemon runs a real agent loop against the mock provider" ;;
    *) echo "  [FAIL] the daemon is not in real mode — no run below would reach the mock"; RC=1 ;;
  esac
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

trust)
  # The owner trust policy is a LOAD gate, so every claim about it needs a
  # fresh load to observe. Restarting also settles the durability question in
  # the same run: the policy is re-derived from `plugins.toml` at construction,
  # so a policy that did not survive a restart would not be a policy.
  say "plant plugin trees"
  python3 "$HERE/plant_plugins.py" "$INSTALLED" "$MARKETPLACES" || exit 1

  say "start server (default posture)"
  start_server || exit 1
  python3 "$HERE/drive_trust.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$ALEPH_HOME" baseline || RC=$?

  say "restart with enforcement on and one plugin vouched for"
  stop_server
  start_server || exit 1
  python3 "$HERE/drive_trust.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$ALEPH_HOME" enforced || RC=$?

  say "restart with the vouch withdrawn"
  stop_server
  start_server || exit 1
  python3 "$HERE/drive_trust.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$ALEPH_HOME" blocked || RC=$?

  say "plugins.toml as the operator would read it"
  cat "$ALEPH_HOME/data/plugins.toml" 2>/dev/null | head -30
  ;;

browse)
  # The built-in marketplace cannot be faked: its content is extracted from the
  # binary into $ALEPH_HOME/plugins/cache/aleph-official on startup, and the
  # bug this scenario pins was a *sentinel* ("bundled") being resolved as a
  # relative path by the lookup side only. A unit test can build the layout;
  # only a real boot proves the extractor, the resolver and the RPC agree.
  say "start server (the bundled extractor populates the built-in marketplace)"
  start_server || exit 1
  ls "$ALEPH_HOME/plugins/cache/aleph-official" 2>/dev/null | head -10 \
    || echo "(no built-in cache extracted — the contents phase will say so)"

  say "drive: browse"
  python3 "$HERE/drive_browse.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" contents || RC=$?

  say "the CLI renders the same contents"
  # A renderer reading keys the server never sends prints a column of dashes,
  # which looks like "no value yet" rather than a bug. Assert a real name
  # reaches stdout.
  # `--server` is the WebSocket URL (`DEFAULT_GATEWAY_URL` is `ws://…/ws`);
  # an `http://` one fails with "URL scheme not supported" before a single
  # frame is sent, which reads like a server problem and is not one.
  CLI_OUT="$("$CLI" --server "ws://127.0.0.1:$GATEWAY_PORT/ws" plugin marketplace browse 2>&1 | head -30)"
  printf '%s\n' "$CLI_OUT"
  if printf '%s' "$CLI_OUT" | grep -q "@aleph-official"; then
    echo "  [PASS] the CLI prints browsed rows with their marketplace"
  else
    echo "  [FAIL] the CLI printed no aleph-official row"; RC=1
  fi

  say "drive: install a name that browsing found"
  python3 "$HERE/drive_browse.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" install || RC=$?
  ls "$INSTALLED" 2>/dev/null | head -10
  ;;

marketplaces)
  # The *registration* surface — a different question from `browse`, which
  # lists a marketplace's contents. `plugin.marketplace.list` was the last
  # member of the family with no contract type (a `json!` literal server-side,
  # a hand-decode client-side), and `add`/`remove` had exactly one client:
  # `interfaces/cli`, a binary `aleph-app-release.yml` never builds. So on a
  # desktop App the whole registration surface was unreachable.
  #
  # Needs a real boot for one specific reason: the built-in marketplace is
  # injected into every `list()` and refused by every `remove()`, and on a
  # fresh install it is the only row on screen. Whether the Panel draws a
  # Remove button on a row the server then rejects is a question only the real
  # manager can answer.
  say "plant a local marketplace to add"
  python3 "$HERE/plant_plugins.py" "$INSTALLED" "$MARKETPLACES" || exit 1

  say "start server (the bundled extractor populates the built-in marketplace)"
  start_server || exit 1

  say "drive: registrations"
  python3 "$HERE/drive_browse.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" \
    registrations "$MARKETPLACES/qa-market" || RC=$?

  say "the CLI renders the same registrations"
  # `marketplace list` used to pretty-print raw JSON, so a renamed key was
  # invisible on both sides. It now decodes through the contract type; assert
  # real columns reach stdout rather than a dump.
  CLI_OUT="$("$CLI" --server "ws://127.0.0.1:$GATEWAY_PORT/ws" plugin marketplace list 2>&1 | head -20)"
  printf '%s\n' "$CLI_OUT"
  if printf '%s' "$CLI_OUT" | grep -q "aleph-official.*\[local\]"; then
    echo "  [PASS] the CLI prints the built-in row with its type column"
  else
    echo "  [FAIL] the CLI printed no typed aleph-official row"; RC=1
  fi
  # The row the remove call refuses must say so where a human reads it.
  if printf '%s' "$CLI_OUT" | grep -q "not removable"; then
    echo "  [PASS] the CLI names the refusal on the row that carries it"
  else
    echo "  [FAIL] the CLI listed a row it cannot remove without saying so"; RC=1
  fi
  ;;

scope)
  # The claim: a plugin's runtime footprint is one scope that mount creates and
  # unmount takes back — observed from OUTSIDE the process, on the tool
  # catalogue a client actually reads. Until this round `enable` after
  # `disable` re-flipped the status and did nothing else (no server, no slash
  # entry), and the boot-only slash registration meant a plugin enabled after
  # boot had no `/command` at all. Two of the three flips below were silent
  # no-ops that reported success.
  #
  # What this proves that `tests/plugin_lifecycle_roundtrip.rs` cannot: the
  # daemon's own boot order (every handle installed before the first
  # `load_all`) and the real tool bridge (an MCP server's tools reaching
  # `tools.catalog` and leaving it). The integration test wires the three
  # handles by hand; here the shipped binary has to.
  say "plant an MCP plugin whose server is the python mock"
  python3 "$HERE/plant_scope.py" "$INSTALLED" "$HERE/mcp_mock_server.py" || exit 1

  say "start server"
  start_server || exit 1

  say "drive: enable → disable → enable"
  python3 "$HERE/drive_scope.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" || RC=$?

  say "the daemon never mounted without a handle (boot order)"
  # A mount that ran before its MCP handle / memory registry / tool catalog was
  # installed logs `effect skipped: handle not attached` and records a Warn
  # diagnostic. On the daemon that is a boot-order regression, not a state.
  #
  # The file is `logs/aleph-server.log.YYYY-MM-DD` (`start/helpers.rs`), so a
  # `*.log` glob matches nothing and a grep over it is green forever. The
  # positive control below is what makes the negative claim mean anything: it
  # proves the grep is reading the file the daemon wrote THIS run.
  if grep -h "plugin mounted, plugin_id=qa-scope" "$ALEPH_HOME"/logs/aleph-server.log.* 2>/dev/null | grep -q .; then
    echo "  [PASS] the server log records the qa-scope mount (the grep below is reading the right file)"
    if grep -h "handle not attached" "$ALEPH_HOME"/logs/aleph-server.log.* 2>/dev/null | grep -q .; then
      echo "  [FAIL] a mount ran before its handle was installed:"
      grep -h "handle not attached" "$ALEPH_HOME"/logs/aleph-server.log.* | head -5; RC=1
    else
      echo "  [PASS] no 'handle not attached' skip in the server log"
    fi
  else
    echo "  [FAIL] no 'plugin mounted, plugin_id=qa-scope' line in $ALEPH_HOME/logs/ — the boot-order grep has nothing to read"
    ls "$ALEPH_HOME/logs" 2>/dev/null; RC=1
  fi
  ;;

panel)
  # BOOTS AND WAITS. Everything below is renderer behaviour, so there is no
  # agent turn -- what the fixture supplies is a realistic registration state
  # (a plantable local marketplace, and the built-in that can never be removed)
  # and a Panel served from disk.
  #
  # The Panel is embedded with `rust_embed`, which reads from disk in debug
  # builds -- so `interfaces/webchat/dist` must exist and be current. Built
  # here rather than assumed: a stale dist renders the previous round, and
  # every assertion below then passes or fails for the wrong reason.
  if [ "${SKIP_BUILD:-0}" != "1" ]; then
    say "build the Panel (debug rust_embed serves dist/ from disk)"
    if ! (cd "$REPO" && HOME="$REAL_HOME" just wasm 2>&1 | tail -5); then
      echo "wasm build failed" >&2; exit 1
    fi
  fi
  [ -f "$REPO/interfaces/webchat/dist/aleph_panel_bg.wasm" ] || {
    echo "no Panel dist -- run: just wasm" >&2; exit 1; }

  say "plant a local marketplace to add"
  python3 "$HERE/plant_plugins.py" "$INSTALLED" "$MARKETPLACES" || exit 1

  say "start server"
  start_server || exit 1

  say "drive: the model-facing verbs (items 8 + 9, no agent turn needed)"
  # The fixture's provider is a dead port on purpose, so an agent turn cannot
  # complete -- and an agent turn is not what these two items claim anyway. The
  # claim is that the tool face and the RPC face answer the same question with
  # the same answer, and that the tool has no install verb. `tools.invoke`
  # reaches the real registry with the real arguments, which is the narrowest
  # thing that can say so.
  python3 "$HERE/drive_tool_face.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" \
    "$MARKETPLACES/qa-market" || RC=$?

  cat <<CHECKLIST

Panel: http://127.0.0.1:$GATEWAY_PORT/   ->  Settings -> Plugins -> Marketplaces
config on disk: $CONFIG
local marketplace to add: $MARKETPLACES/qa-market
server pid: $SERVER_PID

  1. FRESH LIST. The section lists exactly one row: aleph-official, tagged
     'local'. It has NO trash button; in its place is the built-in label, and
     that label's title attribute carries the server's own refusal text (hover,
     or read getAttribute('title')). A Remove button here would be a button the
     server refuses -- which is why 'removable' is a server-derived bit and not
     a client-side comparison against the name 'aleph-official'.

  2. NOT-CONNECTED IS NOT EMPTY. From another shell: kill $SERVER_PID, then
     reload the page. The section must say it is loading/connecting -- NOT
     'no marketplaces registered'. A dropped socket or a refusal rendered as
     'there are none' is the admin_refusal class: only an Ok may assert about
     the thing being read. Re-run this scenario to get the server back.

  3. ADD A LOCAL PATH. Type $MARKETPLACES/qa-market and press Enter.
     A row appears named 'qa-market', tagged 'local', with a trash button.
     Then on disk:  grep -A3 plugin_marketplaces $CONFIG   ->  type = "local".

  4. WINDOWS-SHAPED PATH (the classifier). Add:  C:\dir\mk
     Before this round the RPC called this GITHUB and named it 'c:\dir\mk' --
     a registration no fetch could ever resolve. Now the row (or the error
     banner) must show 'local', the name must be 'mk', and the failure must be
     about a path that does not exist -- not about an invalid GitHub repo.

  5. GITHUB URL IS CANONICALISED. Add:
       https://github.com/aleph-qa-does-not-exist/nope
     The fetch fails (that repo is not real). This is the ONE item that leaves
     the machine, and it fails fast. What matters is what got STORED:
       grep -B2 -A3 aleph-qa-does-not-exist $CONFIG
     ->  source = "aleph-qa-does-not-exist/nope"  (the URL collapsed to the
     slug) and type = "github". Had it been classified Local instead, the error
     would read 'Local marketplace path does not exist: https://...' -- more
     misleading than the message it replaced.

  6. REMOVE THE LAST ONE. Delete every added row until only aleph-official is
     left, then restart the server and reload. They must stay gone. This is the
     DOM half of the 2026-08-20 bug: a section carrying skip_serializing_if
     could not be CLEARED, so removing the last entry reported success and came
     back at the next load.

  7. A BAD SOURCE IS REFUSED, NOT STORED. Add:  ..
     The banner names the refusal, and the config gains no entry for it -- the
     old handler stored anything at all and only failed at sync time.

  8 + 9 already ran above (drive_tool_face.py) -- read its PASS/FAIL lines.
     They cover: both faces list the same registrations with the same
     'removable' bits; marketplace_add registers AND fetches; the same
     classifier answers on the tool face (Windows path -> local, '..'
     refused); a browse row says 'operator_can_install' rather than a bare
     'installable' (a bit named for an action this tool does not have); and
     there is no install action to call while the advertised description says
     both that it cannot install and that registering executes nothing.

probe verdict so far: rc=$RC   (0 = the driven half passed)

Ctrl-C when done (KEEP=1 to retain $QA_ROOT).
CHECKLIST
  # Park in the foreground so the server outlives the checklist.
  while kill -0 "$SERVER_PID" 2>/dev/null; do sleep 5; done
  ;;

visibility)
  # The claim is about what the MODEL is shown, so the oracle is the mock
  # provider's request log — `plugins.list` shows every row regardless (the
  # management face is not project-gated) and cannot tell the two runs apart.
  #
  # Two facts make the "no project" run genuinely project-less: the daemon is
  # started from $QA_ROOT (the CWD fallback then names a directory with no
  # `.aleph/plugins`), and the project is registered through `projects.add`,
  # which is the very producer `collect_plugin_dirs` reads — a plugin planted
  # anywhere else would prove discovery, not visibility.
  PROJECT="$QA_ROOT/proj-vis"
  REQ_LOG="$QA_ROOT/requests.jsonl"
  say "plant a project plugin: one skill, one agent, one MCP server"
  python3 "$HERE/plant_visibility.py" "$PROJECT" "$HERE/mcp_mock_server.py" || exit 1

  say "start mock provider (single-shot plan, recording every request)"
  python3 "$BUSY/mock_anthropic.py" "$MOCK_PORT" /etc/hostname single-shot "" "$REQ_LOG" \
    >"$QA_ROOT/mock.log" 2>&1 &
  MOCK_PID=$!
  for _ in $(seq 1 20); do
    curl -sf -o /dev/null "http://127.0.0.1:$MOCK_PORT/v1/models" 2>/dev/null && break
    sleep 0.5
  done

  say "start server from a non-project directory and register the project"
  SERVER_CWD="$QA_ROOT"
  start_server || exit 1
  python3 "$HERE/drive_visibility.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$PROJECT" "$REQ_LOG" register || RC=$?

  say "restart so discovery walks the registered project's .aleph/plugins"
  # Load-bearing, not caution: `ExtensionManager::ensure_loaded` scans the
  # registered projects once per process, and `projects.add` does not rescan.
  stop_server
  start_server || exit 1
  # A `Simulated` daemon never calls the mock, so every negative arm below
  # would hold vacuously. The drive's controls catch that as a symptom; this
  # line names the cause first.
  MODE_LINE="$(grep "Mode:" "$QA_ROOT/server.log" | tail -1)"
  echo "  ${MODE_LINE:-(no Mode: line in the server log)}"
  case "$MODE_LINE" in
    *"Real AgentLoop"*) echo "  [PASS] the daemon runs a real agent loop against the mock provider" ;;
    *) echo "  [FAIL] the daemon is not in real mode — no run below would reach the mock"; RC=1 ;;
  esac

  say "probe: one run inside the project, one run with no project"
  python3 "$HERE/drive_visibility.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$PROJECT" "$REQ_LOG" probe || RC=$?
  ;;

command)
  # The claim: `/cmd args` puts the command's RENDERED BODY in front of the
  # model. Before this round the body sat in `SkillRegistration.content`,
  # parsed and never read; `/cmd` reached the model as the literal text.
  # The only oracle for "what the model received" is the mock's request log —
  # the persisted history deliberately stays the RAW input (the session title
  # is derived from it), so `chat.history` is asserted the other way round.
  #
  # The turn's caller is an operator (loopback, no credential). Since P4.7c an
  # inline `` !`cmd` `` runs only for an operator, and even then only once its
  # text is approved — so this turn files a PENDING consent entry under the
  # `SlashCommand` event and withholds the command with the operator
  # placeholder. Both are asserted: the placeholder in the request log, the
  # entry on the operator's own review surface (`aleph-server hooks list`).
  # Before the turn the listing is shown to be GATED: disabled, the command is
  # gone; enabled again, it is back — so its presence is not a fixture fact.
  export ALEPH_ACTIVATION_GATE=fatal
  REQ_LOG="$QA_ROOT/requests.jsonl"
  say "plant a plugin with a command"
  python3 - "$INSTALLED" <<'PY' || exit 1
import pathlib, sys
root = pathlib.Path(sys.argv[1], "qa-cmd-plugin")
(root / ".claude-plugin").mkdir(parents=True, exist_ok=True)
(root / "commands").mkdir(exist_ok=True)
(root / ".claude-plugin" / "plugin.json").write_text('{"name": "qa-cmd-plugin", "version": "1.0.0"}\n')
# The inline command's output (QA_INLINE_RAN) is not a substring of its source,
# which the withheld placeholder quotes — so "absent output" means "did not run".
(root / "commands" / "greet.md").write_text(
    "---\ndescription: Greet someone\nargument-hint: \"[name]\"\n---\n"
    "Say hello to $1 and mention the token QA_CMD_MARKER_$1 verbatim.\n"
    "Second: ${2:-nobody}. Inline: !`printf 'QA_%s_RAN' INLINE`\n")
PY
  say "start mock provider (single-shot: every turn ends with no tool call)"
  start_mock /etc/hostname single-shot "" "$REQ_LOG" || exit 1
  say "start server"
  start_server || exit 1
  assert_real_mode
  say "drive"
  python3 "$HERE/drive_command.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$REQ_LOG" || RC=$?
  say "the withheld inline command is pending on the operator's review surface"
  HOOKS_OUT="$("$BIN" hooks list 2>&1)"
  printf '%s\n' "$HOOKS_OUT" | head -12
  if printf '%s\n' "$HOOKS_OUT" | grep -E "pending +qa-cmd-plugin +printf 'QA_%s_RAN' INLINE" >/dev/null; then
    echo "  [PASS] \`aleph-server hooks list\` shows the inline command pending under qa-cmd-plugin"
  else
    echo "  [FAIL] no pending qa-cmd-plugin inline-command entry in \`aleph-server hooks list\`"; RC=1
  fi
  ;;

exit2)
  # The claim: a PreToolUse hook written the Claude Code way —
  # `echo reason >&2; exit 2` — BLOCKS the tool and its stderr reaches the
  # model as the tool result. Before this round the exit code was recorded
  # and never consulted: the hook ran, printed, and the tool ran anyway.
  # The matcher is spelled `Read` (Claude Code's name for `file_read`) so the
  # alias table is on the same wire.
  #
  # Approved the operator's way, not by writing the allowlist by hand: an
  # approval is bound to the root the hook fired from, and `approve` mints
  # none without one — so the hook fires once (un-approved: skipped, recorded
  # pending, the tool runs), then `aleph-server hooks test <fp>` reviews and
  # approves it, and a second run is the one that must be blocked. The first
  # run is the control: the probe content DOES reach the model there, so the
  # second run's absence of it means the hook, not a broken fixture.
  #
  # PINNED, not endorsed (P4.13 review M-2 / O-B): `aleph-server hooks test`
  # REFUSES to review — and so to approve — any command with `; & | $ \` > <`,
  # which is Claude Code's most common hook idiom, unless the operator sets
  # `ALEPH_HOOK_ALLOW_SHELL_METACHARS=1`. The override is named only in that
  # refusal's own text (nothing under docs/). The refusal is asserted first
  # so a change to it is noticed; then the stage approves WITH the override.
  #
  # The hook's stderr (`QA_BLOCK_REASON`) is not a substring of its source
  # (`printf 'QA_%s' …`, review M-1): a message that quoted the command could
  # not pass for the stderr having arrived.
  export ALEPH_ACTIVATION_GATE=fatal
  PROBE="$QA_ROOT/probe.txt"
  printf 'QA_PROBE_CONTENT_MUST_NOT_REACH_THE_MODEL\n' > "$PROBE"
  say "plant a user-level hooks.json (matcher \`Read\`)"
  python3 - "$ALEPH_HOME" <<'PY' || exit 1
import json, pathlib, sys
cmd = "printf 'QA_%s' BLOCK_REASON >&2; exit 2"
pathlib.Path(sys.argv[1], "hooks.json").write_text(json.dumps({"hooks": {"PreToolUse": [
    {"matcher": "Read", "hooks": [{"type": "command", "command": cmd}]}]}}, indent=2))
PY
  python3 -c 'import json,sys; json.dump({"name": "file_read", "input": {"path": sys.argv[1]}}, open(sys.argv[2], "w"))' \
    "$PROBE" "$QA_ROOT/spec.json" || exit 1
  # Started from $QA_ROOT: the daemon's CWD is a hook layer of its own
  # (`<cwd>/.aleph/hooks.json`), and this stage means the user layer only.
  SERVER_CWD="$QA_ROOT"
  say "start mock provider (tool-chain: every turn reads the probe)"
  start_mock "$PROBE" tool-chain "$QA_ROOT/spec.json" "$QA_ROOT/requests-prime.jsonl" || exit 1
  say "start server"
  start_server || exit 1
  assert_real_mode
  say "run 1 — the hook is not approved yet (control)"
  python3 "$HERE/drive_exit2.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$QA_ROOT/requests-prime.jsonl" prime || RC=$?
  say "the operator reviews and approves it"
  HOOKS_OUT="$("$BIN" hooks list 2>&1)"
  printf '%s\n' "$HOOKS_OUT" | head -12
  # O-A (P4.13a): the review surface names the verb that exists.
  if printf '%s\n' "$HOOKS_OUT" | grep -qF 'review with `aleph-server hooks test <fingerprint>`' \
      && ! printf '%s\n' "$HOOKS_OUT" | grep -qF '`aleph hooks'; then
    echo "  [PASS] \`hooks list\` points at \`aleph-server hooks test\`, never bare \`aleph hooks\`"
  else
    echo "  [FAIL] \`hooks list\` does not name \`aleph-server hooks test\` (or still says \`aleph hooks\`)"; RC=1
  fi
  FP="$(printf '%s\n' "$HOOKS_OUT" | awk '$3 == "user:global" && $2 == "pending" {print $1; exit}')"
  if [ -z "$FP" ]; then
    echo "  [FAIL] the Read-matched hook fired no pending user:global entry — the matcher never matched file_read"
    RC=1
  else
    echo "  [PASS] the hook fired on file_read and filed pending entry $FP (matcher \`Read\` reached file_read)"
    # M-2 pin: without the override the review — the only approval path — refuses.
    REFUSE_OUT="$(printf 'y\ny\n' | env -u ALEPH_HOOK_ALLOW_SHELL_METACHARS "$BIN" hooks test "$FP" 2>&1)"
    REFUSE_RC=$?
    STILL="$("$BIN" hooks list 2>/dev/null | awk -v fp="$FP" '$1 == fp {print $2}')"
    if [ "$REFUSE_RC" -ne 0 ] && printf '%s' "$REFUSE_OUT" | grep -q "contains shell metacharacters" \
        && ! printf '%s' "$REFUSE_OUT" | grep -q "Approved $FP" && [ "$STILL" = "pending" ]; then
      echo "  [PASS] pinned (M-2/O-B): without ALEPH_HOOK_ALLOW_SHELL_METACHARS the review refuses this idiom (rc=$REFUSE_RC) and the entry stays pending"
    else
      echo "  [FAIL] the no-override review behaved differently from the pin (rc=$REFUSE_RC, status=$STILL) — a change to notice:"
      printf '%s\n' "$REFUSE_OUT" | sed 's/^/    /' | head -8; RC=1
    fi
    APPROVE_OUT="$(printf 'y\ny\n' | ALEPH_HOOK_ALLOW_SHELL_METACHARS=1 "$BIN" hooks test "$FP" 2>&1)"
    printf '%s\n' "$APPROVE_OUT" | sed 's/^/    /'
    if printf '%s' "$APPROVE_OUT" | grep -q "Approved $FP"; then
      echo "  [PASS] \`aleph-server hooks test $FP\` approved it"
    else
      echo "  [FAIL] \`aleph-server hooks test $FP\` did not approve it"; RC=1
    fi
  fi
  say "run 2 — approved: the hook must block (no restart: approvals are re-read by stamp)"
  start_mock "$PROBE" tool-chain "$QA_ROOT/spec.json" "$QA_ROOT/requests.jsonl" || exit 1
  python3 "$HERE/drive_exit2.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$QA_ROOT/requests.jsonl" block || RC=$?
  ;;

subagent)
  # The claim (P4.13 ruling 5, optional): a plugin command whose
  # `allowed-tools` restricts the turn to a list hands a `subagent` it spawns
  # the SAME restricted view, not the full catalogue — otherwise one delegation
  # undoes the restriction (判据 §14: two legal steps, together equivalent to
  # the forbidden one). The oracle is the tools[] of each request the mock
  # received: the parent turn's (carrying the `<command>` block) and the
  # child's (whose first user message is the delegated task). A control
  # asserts the parent's view really is restricted, so the child comparison
  # cannot pass against an unrestricted parent.
  export ALEPH_ACTIVATION_GATE=fatal
  REQ_LOG="$QA_ROOT/requests.jsonl"
  say "plant a plugin whose command restricts the turn to Read + Task"
  python3 - "$INSTALLED" <<'PY' || exit 1
import pathlib, sys
root = pathlib.Path(sys.argv[1], "qa-sub-plugin")
(root / ".claude-plugin").mkdir(parents=True, exist_ok=True)
(root / "commands").mkdir(exist_ok=True)
(root / ".claude-plugin" / "plugin.json").write_text('{"name": "qa-sub-plugin", "version": "1.0.0"}\n')
# `Read` → file_read, `Task` → subagent (the CC alias table).
(root / "commands" / "delegate.md").write_text(
    "---\ndescription: Delegate a read\nallowed-tools: Read, Task\n---\n"
    "Delegate the task to a sub-agent. QA_SUB_PARENT_MARKER\n")
PY
  # Every tool turn delegates; the child's first user message is the task.
  # `coder`, not the default child: it names `bash` in its own allowlist, so
  # "restricted" and "unrestricted" differ by one named tool, and the control
  # phase proves an unrestricted parent's coder child does carry it.
  python3 -c 'import json,sys; json.dump({"name": "subagent", "input": {"task": "QA_SUB_CHILD_TASK read nothing and stop", "agent_type": "coder"}}, open(sys.argv[1], "w"))' \
    "$QA_ROOT/spec.json" || exit 1
  say "start mock provider (tool-chain: every tool turn calls subagent)"
  start_mock /etc/hostname tool-chain "$QA_ROOT/spec.json" "$REQ_LOG" || exit 1
  say "start server"
  start_server || exit 1
  assert_real_mode
  say "drive: /cmd delegates (the claim)"
  python3 "$HERE/drive_subagent.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$REQ_LOG" restricted || RC=$?
  say "control: a plain turn delegates (its child must carry the full view)"
  start_mock /etc/hostname tool-chain "$QA_ROOT/spec.json" "$QA_ROOT/requests-control.jsonl" || exit 1
  python3 "$HERE/drive_subagent.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" "$QA_ROOT/requests-control.jsonl" control || RC=$?
  ;;

cc-cache)
  # The claim: a plugin Claude Code installed under ~/.claude/plugins is
  # discovered, listed with origin `claude_cache`, DISABLED until the operator
  # enables it, its MCP server (declared the Claude Code way — a `.mcp.json`
  # beside the manifest, NO `aleph.runtime`, P4.15) mounts on enable, and
  # nothing under ~/.claude is written. $HOME is the scratch root here
  # (qa_redirect_home), so the fixture IS ~/.claude for the server.
  #
  # The model may NOT enable it (P4.10 D-A: `plugin_manage enable` refuses a
  # `claude_cache` row); the operator does, through `plugins.enable` — what
  # `aleph plugin enable` and the Panel call. Both are driven, in that order.
  #
  # Also pinned (P4.11 R2-I2 — accepted compat cost, P4.14 DEVIATION row;
  # candidate plugin-resource face in the final review): the model's
  # `file_read` of a file under the CC plugin root is denied, because
  # `~/.claude/plugins` is a pre-grant root (`utils::paths::pregrant_roots`).
  # Asserted as it is today so that a change is noticed, not as a goal.
  export ALEPH_ACTIVATION_GATE=fatal
  CC_HOME="$HOME/.claude"
  CC_ROOT="$CC_HOME/plugins/cache/qa-market/qa-cc/1.0.0"
  say "plant a Claude Code plugin cache"
  python3 - "$CC_HOME" "$HERE/mcp_mock_server.py" <<'PY' || exit 1
import json, pathlib, sys
plugins = pathlib.Path(sys.argv[1], "plugins")
mock = pathlib.Path(sys.argv[2]).resolve()   # the `scope` stage's stdio MCP mock
root = plugins / "cache" / "qa-market" / "qa-cc" / "1.0.0"
(root / ".claude-plugin").mkdir(parents=True, exist_ok=True)
(root / "commands").mkdir(exist_ok=True)
(root / ".claude-plugin" / "plugin.json").write_text('{"name": "qa-cc", "version": "1.0.0", "description": "CC-installed"}\n')
(root / ".mcp.json").write_text(json.dumps({"mcpServers": {"mock": {"command": sys.executable, "args": [str(mock)]}}}))
(root / "commands" / "hello.md").write_text("---\ndescription: hi\n---\nSay hi to $ARGUMENTS. QA_CC_COMMAND_BODY\n")
# The shape Claude Code writes: `installPath` is the absolute cache dir on the
# machine that wrote it. Aleph derives the dir from key + version and never
# follows this field (`discovery/claude_cache.rs`); it is written true anyway.
(plugins / "installed_plugins.json").write_text(json.dumps({"version": 2, "plugins": {
    "qa-cc@qa-market": [{"scope": "user", "installPath": str(root), "version": "1.0.0",
                         "installedAt": "2026-01-01T00:00:00.000Z", "lastUpdated": "2026-01-01T00:00:00.000Z"}]}}, indent=2))
PY
  # Phase `first`'s model turn calls `plugin_manage enable` (the model face of
  # the refusal); phase `second`'s calls `file_read` under the plugin root.
  python3 -c 'import json,sys; json.dump({"name": "plugin_manage", "input": {"action": "enable", "name": "qa-cc"}}, open(sys.argv[1], "w"))' \
    "$QA_ROOT/spec-enable.json" || exit 1
  python3 -c 'import json,sys; json.dump({"name": "file_read", "input": {"path": sys.argv[1]}}, open(sys.argv[2], "w"))' \
    "$CC_ROOT/commands/hello.md" "$QA_ROOT/spec.json" || exit 1
  # "Nothing written" is read two ways (review M-3): mtimes (`find -newer`)
  # and a content digest of every path under ~/.claude — the digest also
  # sees a write that keeps its mtime and a tree that vanished.
  cc_digest() {
    python3 - "$CC_HOME" <<'PY'
import hashlib, os, sys
root = sys.argv[1]
if not os.path.isdir(root):
    print("MISSING"); sys.exit(0)
h = hashlib.sha256()
for d, dirs, files in sorted(os.walk(root)):
    dirs.sort()
    h.update(os.path.relpath(d, root).encode() + b"/\0")
    for f in sorted(files):
        p = os.path.join(d, f)
        h.update(os.path.relpath(p, root).encode() + b"\0")
        h.update(open(p, "rb").read() if not os.path.islink(p) else os.readlink(p).encode())
print(h.hexdigest())
PY
  }
  CC_BEFORE="$(cc_digest)"
  MARK="$QA_ROOT/.mark"; touch "$MARK"; sleep 1
  say "start mock provider (tool-chain: the model tries plugin_manage enable)"
  start_mock /etc/hostname tool-chain "$QA_ROOT/spec-enable.json" "$QA_ROOT/requests-first.jsonl" || exit 1
  say "start server"
  start_server || exit 1
  assert_real_mode
  say "drive (discovered, disabled, model refused on both faces, operator enables → command + MCP tool appear)"
  python3 "$HERE/drive_cc_cache.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" first "$QA_ROOT/requests-first.jsonl" \
    "$SERVER_PID" "$HERE/mcp_mock_server.py" || RC=$?
  say "restart (the enable is durable; boot mounts the command and the server again)"
  stop_server
  start_mock /etc/hostname tool-chain "$QA_ROOT/spec.json" "$QA_ROOT/requests.jsonl" || exit 1
  start_server || exit 1
  python3 "$HERE/drive_cc_cache.py" "ws://127.0.0.1:$GATEWAY_PORT/ws" second "$QA_ROOT/requests.jsonl" \
    "$SERVER_PID" "$HERE/mcp_mock_server.py" || RC=$?
  say "the enable bit lives in Aleph's plugins.toml"
  # Parsed, not grepped (review M-6): a neighbour's `enabled = true` within a
  # few lines must not answer for qa-cc.
  TOML_OUT="$(python3 - "$ALEPH_HOME/data/plugins.toml" <<'PY'
import sys, tomllib
try:
    doc = tomllib.load(open(sys.argv[1], "rb"))
except (OSError, tomllib.TOMLDecodeError) as e:
    print(f"unreadable: {e}"); sys.exit(1)
entry = (doc.get("entries") or {}).get("qa-cc")
print(f"entries.qa-cc = {entry}")
sys.exit(0 if isinstance(entry, dict) and entry.get("enabled") is True else 1)
PY
)"
  if [ $? -eq 0 ]; then
    echo "  [PASS] plugins.toml records qa-cc enabled — $TOML_OUT"
  else
    echo "  [FAIL] plugins.toml does not record qa-cc enabled — $TOML_OUT"; RC=1
  fi
  say "nothing under ~/.claude changed"
  CHANGED="$(find "$CC_HOME" -newer "$MARK" 2>&1 | grep -v '^$' || true)"
  if [ -z "$CHANGED" ]; then echo "  [PASS] no path under $CC_HOME has a newer mtime"; else echo "  [FAIL] written under ~/.claude:"; echo "$CHANGED"; RC=1; fi
  CC_AFTER="$(cc_digest)"
  if [ "$CC_BEFORE" = "$CC_AFTER" ] && [ "$CC_AFTER" != "MISSING" ]; then
    echo "  [PASS] the content digest of $CC_HOME is unchanged (${CC_AFTER:0:16}…)"
  else
    echo "  [FAIL] the content of $CC_HOME changed: before=${CC_BEFORE:0:16} after=${CC_AFTER:0:16}"; RC=1
  fi
  ;;
*)
  echo "unknown scenario '$SCENARIO' (manifest | scaffold | trust | browse | marketplaces | scope | panel | visibility | command | exit2 | subagent | cc-cache)" >&2; exit 2;;
esac

say "server warnings about plugins"
# `aleph-server.log.YYYY-MM-DD`, never `*.log`: the previous glob matched
# nothing, so this section had printed "no warnings" for every stage it ever ran.
grep -i "plugin" "$ALEPH_HOME"/logs/aleph-server.log.* 2>/dev/null | grep -iE "warn|error" | head -20

say "verdict: rc=$RC"
exit "$RC"
