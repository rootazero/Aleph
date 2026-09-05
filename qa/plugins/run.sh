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
#   ./qa/plugins/run.sh panel      # BOOTS AND WAITS: the same surface through
#                                  # the browser, plus the source classifier
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
*)
  echo "unknown scenario '$SCENARIO' (manifest | scaffold | trust | browse | marketplaces | panel)" >&2; exit 2;;
esac

say "server warnings about plugins"
grep -i "plugin" "$ALEPH_HOME"/logs/*.log 2>/dev/null | grep -iE "warn|error" | head -20

say "verdict: rc=$RC"
exit "$RC"
