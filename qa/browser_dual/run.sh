#!/usr/bin/env bash
# Real-machine QA for the dual-engine browser stack (obscura + chromium over
# Aleph's own CDP driver).
#
#   ./qa/browser_dual/run.sh provision  # the ledger installs obscura from the
#                                       # REAL release, sha256-checked; then the
#                                       # asset mirror is pointed at a dead host
#                                       # and the checksum host does NOT follow
#   ./qa/browser_dual/run.sh open       # the readiness gate: /json/version AND a
#                                       # navigate; argv and storage-dir as launched
#   ./qa/browser_dual/run.sh snapshot   # engine=obscura header, refs for the
#                                       # elements that have them, none for the ones
#                                       # obscura cannot see
#   ./qa/browser_dual/run.sh click      # by ref AND by coordinates, each changing
#                                       # the URL; a pre-navigation ref goes stale
#   ./qa/browser_dual/run.sh stall      # a spinning page yields a "did not answer"
#                                       # refusal and the engine is still alive
#   ./qa/browser_dual/run.sh reap       # kill -9 the server: BOTH engines' orphans
#                                       # are swept, a prefix-neighbour is NOT
#   ./qa/browser_dual/run.sh caps       # the capability table, probed against the
#                                       # real binary, verb by verb
#   ./qa/browser_dual/run.sh switch     # a cookie set on obscura is readable on
#                                       # chromium afterwards, the tab is back on
#                                       # its URL, and the obscura process is GONE
#
# Same scratch-HOME discipline as qa/browser_managed/run.sh. No mock provider in
# any stage: every claim goes through `tools.invoke`, which runs a tool without
# an agent turn — but the config still carries a FAKE API KEY, because a server
# in `Mode: Simulated` answers `tools.invoke` with a boot-phase placeholder that
# reads exactly like a wiring failure (qa/README.md).
#
# Two real things, named up front rather than discovered at minute three:
#   * an obscura binary (ALEPH_QA_OBSCURA) — every stage but `provision` pins it
#     so the run never depends on a network install;
#   * a Chrome (ALEPH_QA_CHROME) for the stages that need the OTHER engine.
# `provision` additionally needs the network, and checks for it before building.
set -uo pipefail

STAGE="${1:-open}"
case "$STAGE" in
  provision|open|snapshot|click|stall|reap|caps|switch) ;;
  *) echo "unknown stage: $STAGE" >&2; exit 64 ;;
esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
SHARED="$HERE/../busy_input"
MANAGED="$HERE/../browser_managed"

# `add_browser_config.py --cli-binary` is `required=True` and this fixture
# shares that script, so it must supply one even though it drives CDP and never
# invokes the CLI. Resolved exactly the way `qa/browser_managed/run.sh` does —
# same env var, same 69 — rather than relaxing a flag another fixture depends
# on being mandatory.
CLI="${PLAYWRIGHT_CLI:-$(command -v playwright-cli 2>/dev/null)}"
if [ -z "$CLI" ]; then
  echo "no playwright-cli on PATH; set PLAYWRIGHT_CLI=/path/to/playwright-cli" >&2
  echo "(this fixture drives CDP and never runs it — it is required only because" >&2
  echo " add_browser_config.py, shared with qa/browser_managed, demands the key)" >&2
  exit 69
fi

# The ledger's own install path is the FIRST place to look, because that is
# where `provision` puts one and where `runtime_manage` would find it. A bare
# `command -v obscura` finds nothing on a machine that installed it through
# Aleph — obscura is never put on PATH — and the fixture would then have
# refused to run beside a perfectly good binary.
OBSCURA_TAG="v0.2.2"
OBSCURA_REPO="h4ckf0r0day/obscura"
# `$HOME` is still the operator's own home here — `qa_redirect_home` runs much
# further down, and this lookup has to read the machine's real ledger.
LEDGER_OBSCURA="$HOME/.aleph/runtimes/obscura/$OBSCURA_TAG/obscura"
OBSCURA_BIN="${ALEPH_QA_OBSCURA:-}"
if [ -z "$OBSCURA_BIN" ] && [ -x "$LEDGER_OBSCURA" ]; then OBSCURA_BIN="$LEDGER_OBSCURA"; fi
if [ -z "$OBSCURA_BIN" ]; then OBSCURA_BIN="$(command -v obscura 2>/dev/null)"; fi
if [ -z "$OBSCURA_BIN" ] || [ ! -x "$OBSCURA_BIN" ]; then
  echo "no obscura binary; set ALEPH_QA_OBSCURA=/path/to/obscura" >&2
  echo "looked at: \$ALEPH_QA_OBSCURA, $LEDGER_OBSCURA, and PATH" >&2
  echo "(install one with runtime_manage{action:\"install\", capability:\"obscura\"}," >&2
  echo " which is what ./qa/browser_dual/run.sh provision exercises)" >&2
  exit 69
fi

CHROME_BIN="${ALEPH_QA_CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
# `open` and `reap` are the two stages whose claims are ABOUT the other engine:
# `open` proves the readiness gate holds for both (spec §7.4), `reap` proves the
# sweep reaches both engines' orphans. Without a Chrome neither can say anything
# about chromium, and a stage that passes by having nothing to check is the
# shape qa/README.md warns about.
if { [ "$STAGE" = "reap" ] || [ "$STAGE" = "open" ] || [ "$STAGE" = "switch" ]; } \
   && [ ! -x "$CHROME_BIN" ]; then
  echo "no browser at $CHROME_BIN; set ALEPH_QA_CHROME" >&2
  echo "(this stage's claims are about BOTH engines; without a chromium it would" >&2
  echo " pass by having nothing to check. reap needs a LIVE chromium to spare;" >&2
  echo " switch needs one to switch TO — without it the stage would fail at the" >&2
  echo " launch and prove nothing about the migration)" >&2
  exit 69
fi

# `provision` is the one stage that installs over the real network, and it is
# the network that makes it meaningful: `ReleaseSource::for_runtime` pins the
# release METADATA — and therefore the expected sha256 — at api.github.com, and
# `configured_host` ignores any download_host that is not https. So there is no
# local fixture release this installer can be pointed at, and an unreachable
# GitHub is "cannot run", not "failed".
if [ "$STAGE" = "provision" ]; then
  # Deliberately UNAUTHENTICATED, even if the operator has a `GITHUB_TOKEN` in
  # the environment: `install_release` sends no token, so a preflight that used
  # one would answer a question the installer is not about to ask, and a run
  # that passed the gate would then fail inside the stage for a reason the gate
  # had just declared fine (判据 §18 — the instrument must measure the thing).
  # Headers off the SAME request, not a second call to /rate_limit: the budget
  # is what is being reported, so spending another unit of it to ask about it
  # is both wasteful and — measured — unreliable, because that endpoint answers
  # 403 too once the budget is gone, and the gate then printed "unreadable"
  # while holding the answer in its own response headers.
  META_HEAD="$(curl -s -o /dev/null -D - --max-time 25 \
      "https://api.github.com/repos/$OBSCURA_REPO/releases/tags/$OBSCURA_TAG" 2>/dev/null)"
  META_CODE="$(printf '%s' "$META_HEAD" | awk 'NR==1 {print $2}')"
  if [ "$META_CODE" != "200" ]; then
    echo "cannot read the obscura release metadata on api.github.com (HTTP ${META_CODE:-no answer})" >&2
    # A spent budget is NOT the same problem as being offline: it is 60
    # unauthenticated requests per HOUR per host, shared with everything else on
    # this machine. Saying which, and when it clears, is the difference between
    # "wait" and "debug your network".
    REMAINING="$(printf '%s' "$META_HEAD" | tr -d '\r' \
        | awk 'tolower($1) == "x-ratelimit-remaining:" {print $2}')"
    RESET="$(printf '%s' "$META_HEAD" | tr -d '\r' \
        | awk 'tolower($1) == "x-ratelimit-reset:" {print $2}')"
    if [ -n "$RESET" ]; then
      echo "api.github.com budget: ${REMAINING:-?} left, resets at $(python3 -c \
        "import datetime,sys;print(datetime.datetime.fromtimestamp(int(sys.argv[1]),datetime.timezone.utc).isoformat())" \
        "$RESET" 2>/dev/null || echo "epoch $RESET")" >&2
    fi
    echo "(the installer takes its expected sha256 from there and from nowhere else —" >&2
    echo " ReleaseSource::for_runtime pins the metadata host and configured_host" >&2
    echo " discards a non-https mirror — so this stage cannot be run offline or on a" >&2
    echo " spent budget. See the provision entry in qa/README.md.)" >&2
    exit 69
  fi
fi

QA_ROOT="${QA_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-dual-XXXXXX")}"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18787}"
PAGE_PORT="${PAGE_PORT:-18888}"
DEAD_MOCK_PORT="${DEAD_MOCK_PORT:-18999}"
# The asset mirror `provision`'s second half points at: a syntactically valid
# https host that nothing answers on. https:// on purpose — `configured_host`
# discards a non-https mirror with a warning and falls back to the default, so
# an http:// value here would make the second half measure the DEFAULT host.
DEAD_MIRROR="${DEAD_MIRROR:-https://127.0.0.1:1/gh}"
MARKER="aleph-qa-dual-${RANDOM}${RANDOM}"
# The stall stage needs the per-command timeout to be shorter than the window a
# spinning page blocks the connection for. Measured on this machine: a command
# issued 1.5 s after the navigate answered 4.32 s later. 2 s leaves margin at
# both ends, and is inside the 1..=59 range `BrowserSystemConfig::validate`
# enforces (obscura's own 60 s guillotine is the ceiling).
#
# Overridable because raising it ABOVE the stall is this stage's RED control:
# `STALL_TIMEOUT_SECS=30 ./qa/browser_dual/run.sh stall` lets the wedged call
# outlive the block and succeed, and `…did not report success` goes red. A
# control an operator can run without editing a file is one they will actually
# run.
STALL_TIMEOUT_SECS="${STALL_TIMEOUT_SECS:-2}"

SERVER_PID=""
PAGE_PID=""

say() { printf '\n=== %s ===\n' "$*"; }

cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  # `reap` kills the server and starts a REPLACEMENT, whose pid this shell has
  # never seen. The driver records it the moment it spawns, so it is reaped
  # here whether the stage passed, failed or died half-way — without this the
  # next run meets `Address already in use` at config generation, which reads
  # like a port-choice problem three steps from the cause.
  if [ -f "$QA_ROOT/server2.pid" ]; then
    kill "$(cat "$QA_ROOT/server2.pid")" 2>/dev/null
  fi
  [ -n "$PAGE_PID" ] && kill -9 "$PAGE_PID" 2>/dev/null
  sleep 1
  [ -n "$SERVER_PID" ] && kill -9 "$SERVER_PID" 2>/dev/null
  if [ -f "$QA_ROOT/server2.pid" ]; then
    kill -9 "$(cat "$QA_ROOT/server2.pid")" 2>/dev/null
  fi
  # Anything this run launched and the server did not reap. `--` is required:
  # BSD pgrep reads a leading `-` in the pattern as an option, and the run then
  # leaves live browsers behind while reporting nothing.
  pkill -9 -f -- "--storage-dir=$QA_ROOT" 2>/dev/null
  pkill -9 -f -- "--user-data-dir=$QA_ROOT" 2>/dev/null
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

. "$HERE/../lib/scratch_home.sh"
. "$HERE/../lib/build.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
PAGEDIR="$QA_ROOT/page"
CWD="$QA_ROOT/cwd"
# Named here and passed to every consumer, rather than left to
# `manager.rs:531-535`'s derived default: `reap` has to `pgrep` for these exact
# strings, and a directory the fixture did not choose is one it cannot name.
OBSCURA_STORAGE="$QA_ROOT/obscura-store"
CHROMIUM_UDD="$QA_ROOT/chromium-profile"
mkdir -p "$PAGEDIR" "$CWD" "$CHROMIUM_UDD" "$OBSCURA_STORAGE"
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

TARGET_DIR="$(cd "$REPO" && HOME="$REAL_HOME" cargo metadata --no-deps --format-version 1 2>/dev/null \
  | python3 -c 'import sys,json; print(json.load(sys.stdin)["target_directory"])' 2>/dev/null)"
BIN="${TARGET_DIR:-$REPO/target}/debug/aleph-server"

say "build ($STAGE)"
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  qa_build --bin aleph-server || { echo "build failed" >&2; exit 1; }
fi
[ -x "$BIN" ] || { echo "no binary at $BIN" >&2; exit 1; }

say "serve the page fixture on 127.0.0.1:$PAGE_PORT"
for f in index second stall caps; do
  sed -e "s/__MARKER__/$MARKER/g" "$HERE/pages/$f.html" > "$PAGEDIR/$f.html"
done
(cd "$PAGEDIR" && python3 -m http.server "$PAGE_PORT" --bind 127.0.0.1) >"$QA_ROOT/page.log" 2>&1 &
PAGE_PID=$!
for _ in $(seq 1 30); do curl -sf -o /dev/null "http://127.0.0.1:$PAGE_PORT/index.html" && break; sleep 0.3; done

say "generate a baseline config"
timeout 25 "$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; wait "$GEN_PID" 2>/dev/null
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -20 "$QA_ROOT/gen.log"; exit 1; }

say "patch config"
python3 "$SHARED/patch_config.py" "$CONFIG" \
  --gateway-port "$GATEWAY_PORT" --mock-port "$DEAD_MOCK_PORT" || exit 1
BROWSER_CFG_ARGS=(
  --cli-binary "$CLI"
  --user-data-dir "$OBSCURA_STORAGE"
  --headless true
  # Both stated, never inherited: this fixture's whole subject is the default
  # engine, and a run that followed the product default would be measuring
  # whatever the product currently does rather than what it promises.
  --driver cdp
  --engine obscura
  # None. `extra_args` is prepended to the ENGINE's own argv, and obscura exits
  # 2 on an unrecognised one — the historical `--disable-gpu` killed every
  # launch on this fixture's first run ("unexpected argument '--disable-gpu'
  # found", pid dead before /json/version).
  --default-extra-args ""
)
# Every stage but `provision` needs an obscura that is already there; pinning it
# is what keeps those stages from depending on a network install. `provision` is
# the one stage that must NOT have the pin, or it would test nothing.
if [ "$STAGE" != "provision" ]; then
  BROWSER_CFG_ARGS+=(--obscura-binary-path "$OBSCURA_BIN")
fi
if [ "$STAGE" = "stall" ]; then
  BROWSER_CFG_ARGS+=(--cdp-command-timeout-secs "$STALL_TIMEOUT_SECS")
fi
if [ "$STAGE" = "reap" ] || [ "$STAGE" = "open" ]; then
  # A second profile on the OTHER engine. `open` needs it because spec §7.4 says
  # the readiness gate is proven 对两引擎都成立, and a stage that only ever
  # launches obscura says nothing about the other engine's gate or about
  # `--use-mock-keychain`, whose absence is round-7's headline defect (a Chrome
  # that answers /json/version while every first navigation silently dies).
  # `reap` needs it because the sweep's census was extended to both engines and
  # this is the only place either engine's arm runs on a real machine.
  BROWSER_CFG_ARGS+=(
    --profile-engine "escape=chromium"
    --profile-user-data-dir "escape=$CHROMIUM_UDD"
    --runtime-binary-path "$CHROME_BIN"
    --prefer-system-browser false
  )
fi
# `switch` needs no second profile — it moves the DEFAULT one — but it does need
# a chromium binary to move it onto, and the same pin for the same reason as
# above: so the stage never depends on which browsers this machine happens to
# have. `--prefer-system-browser false` makes the pin the one that is used
# rather than a fallback a system Chrome would win over.
if [ "$STAGE" = "switch" ]; then
  BROWSER_CFG_ARGS+=(
    --runtime-binary-path "$CHROME_BIN"
    --prefer-system-browser false
  )
fi
python3 "$MANAGED/add_browser_config.py" "$CONFIG" "${BROWSER_CFG_ARGS[@]}" || exit 1

# `$HOME/.aleph`, NOT `$ALEPH_HOME` — and the two are equal in this run, which
# is exactly why the spelling has to be chosen rather than picked.
# `ConfigApprovalPolicy::config_path` derives this path from `dirs::home_dir()`,
# so `$HOME` is the fact and `$ALEPH_HOME` is a string that happens to agree
# with it today.
#
# The key names are the snake_case `ActionType` variants (`approval/types.rs`),
# not tool names — and a key that names no variant is SILENTLY IGNORED, so a
# hopeful `"browser_snapshot"` or `"runtime_manage"` here would read like a
# gate that was opened and would in fact be a line nothing consults. Neither
# verb has an `ActionType`, so neither needs one (判据 §17).
#
# A policy file REPLACES the curated defaults wholesale (`ConfigApprovalPolicy`
# `::load_from`), so every variant a stage exercises has to be listed here —
# an absent key is "no policy configured", which a non-interactive run reads as
# a refusal. `browser_cookies_write` was missing until the `switch` stage became
# the first to call `browser_cookies`, and it failed three claims about the
# MIGRATION for a reason that was upstream of it.
#
# There is deliberately NO `browser_switch_engine` key, and its absence is a
# claim. `ActionType`'s own doc promises that an operator who wrote
# `browser_open` covers the switch without a second key, and `decide` consults
# `inherited_from` before it answers "no policy configured". This fixture is the
# only place that promise is exercised against a running server — writing the
# redundant key would retire the one real-machine test of it.
cat > "$HOME/.aleph/approval-policy.json" <<'JSON'
{"defaults":{
  "browser_open":"allow","browser_navigate":"allow","browser_click":"allow",
  "browser_type":"allow","browser_evaluate":"allow","browser_dialog":"allow",
  "browser_session_state":"allow","browser_cookies_write":"allow"
},"allowlist":[],"blocklist":[]}
JSON

say "start server (cwd=$CWD)"
(cd "$CWD" && "$BIN" start) >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 90); do
  curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" && break
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
echo "gateway up on $GATEWAY_PORT"

say "drive: $STAGE"
RC=0
if [ "$STAGE" = "caps" ]; then
  python3 "$HERE/caps.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" \
    --page-url "http://127.0.0.1:$PAGE_PORT/caps.html" \
    --aleph-home "$ALEPH_HOME" \
    --obscura-binary "$OBSCURA_BIN" || RC=$?
elif [ "$STAGE" = "switch" ]; then
  python3 "$HERE/drive_switch.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" \
    --page-url "http://127.0.0.1:$PAGE_PORT/index.html" \
    --aleph-home "$ALEPH_HOME" \
    --obscura-storage "$OBSCURA_STORAGE" || RC=$?
else
  python3 "$HERE/drive.py" \
    "ws://127.0.0.1:$GATEWAY_PORT/ws" "$STAGE" \
    --page-url "http://127.0.0.1:$PAGE_PORT/index.html" \
    --stall-url "http://127.0.0.1:$PAGE_PORT/stall.html" \
    --second-url "http://127.0.0.1:$PAGE_PORT/second.html" \
    --marker "$MARKER" \
    --qa-root "$QA_ROOT" \
    --aleph-home "$ALEPH_HOME" \
    --config "$CONFIG" \
    --release-repo "$OBSCURA_REPO" \
    --release-tag "$OBSCURA_TAG" \
    --dead-mirror "$DEAD_MIRROR" \
    --obscura-binary "$OBSCURA_BIN" \
    --server-bin "$BIN" \
    --server-cwd "$CWD" \
    --gateway-port "$GATEWAY_PORT" \
    --server-pid "$SERVER_PID" \
    --stall-timeout "$STALL_TIMEOUT_SECS" \
    --obscura-storage "$OBSCURA_STORAGE" \
    --chromium-udd "$CHROMIUM_UDD" || RC=$?
  # `reap` kills and restarts the server itself, so it owns the pid from that
  # point on. Cleared ONLY on a clean exit: clearing it unconditionally orphans
  # the server (and every browser it launched) whenever the driver dies before
  # the restart.
  if [ "$STAGE" = "reap" ] && [ "$RC" -eq 0 ]; then
    SERVER_PID=""
  fi
fi

say "server log tail"
LOGDIR="$ALEPH_HOME/logs"
if [ -d "$LOGDIR" ]; then
  grep -iE "obscura|engine|browser|reap" "$LOGDIR"/aleph-server.log* 2>/dev/null | tail -25
else
  tail -25 "$QA_ROOT/server.log"
fi

say "verdict: rc=$RC"
exit "$RC"
