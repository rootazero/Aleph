#!/usr/bin/env bash
# Real-machine QA for the generation request-timeout knob (FEATURE_LOCATOR
# §3.6 / §4.9; the round that made `timeout_seconds` an `Option<u64>`).
#
#   ./qa/generation_timeout/run.sh cap         # a configured cap really cuts a real request
#   ./qa/generation_timeout/run.sh auto        # "unset" lets a request outlive the 8s window
#   ./qa/generation_timeout/run.sh deploy      # ~/.aleph/defaults.toml still reaches the client
#   ./qa/generation_timeout/run.sh precedence  # explicit config outranks the deployment override
#   ./qa/generation_timeout/run.sh panel       # boot + hold, for the Panel's Auto checkbox
#
# ## What `auto` measures, exactly
#
# It watches for 8 s and requires the request to still be open. That falsifies
# "unset collapsed into some short cap", which is the failure this round could
# have introduced. It does NOT distinguish 120 s from the provider's own tuned
# default, because separating those means waiting two minutes for one bit. The
# claim "each provider keeps ITS OWN default" is carried by
# `http::WithRequestTimeout::with_timeout`'s `None` arm and its unit test, not
# by this phase (判据 §18 — a number carries the predicate it measured).
#
#   KEEP=1 …        keep the scratch dir for post-mortem
#   SKIP_BUILD=1 …  reuse the server binary already built
#
# ## What in-process tests cannot say here
#
# `factory.rs`'s guard counts occurrences of `.with_timeout(config.request_timeout_secs())`
# in its own source text, and `http.rs`'s tests build a client and read the
# duration back. Both are assertions about a source file. Neither can say that
# a configured second ever reaches a socket — and the defect this round fixed
# was exactly that gap: the knob was parsed, validated, surfaced in the Panel
# and applied in 2 of 19 arms, with every test green for three rounds.
#
# So the oracle here is a mock HTTP endpoint that never answers, and the
# question asked of it is "did the server hang up on you, and when".
#
# ## Why `provider_type = "openai"` and not `openai_compat`
#
# `openai_compat` was ALREADY one of the four sites that applied the knob
# before this round. A fixture built on it is green against the pre-round tree,
# which makes its green worth nothing (判据 §2 — in what case does this go
# red?). The `"openai" | "openai_image" | "dalle"` arm was one of the fifteen
# that discarded it, so that is the arm under test.
#
# ## What this does NOT cover
#
# * The other eighteen factory arms. They share one executor
#   (`http::WithRequestTimeout`) and `factory.rs`'s source-level guard counts
#   the call sites; this fixture proves the executor works on a real socket,
#   once, and says nothing about whether arm #14 calls it.
# * `voice_http_client` and `DEFAULT_LOCAL_VOICE_TIMEOUT_SECS` — the BYO local
#   speech path builds its own client and is not reachable through
#   `image_generate`. That constant remains unmeasured, and its own doc
#   comment says so.
# * Whether 120 (or any number) is a GOOD timeout for anything. This fixture
#   only shows that the number an operator writes is the number that bounds
#   each attempt.
#
# ## `timeout_seconds` is PER ATTEMPT, and the Panel does not say so
#
# Measured 2026-09-06 in the `cap` phase, on a binary built from the tree it
# was measuring (see the staleness guard below -- an earlier reading of this
# same line came from a binary that was not): a 2 s cap produced THREE aborted
# attempts of ~2 s each and a tool call that settled at ~7 s. The provider
# retries a timeout, so the operator's wall-clock wait is roughly
# `timeout_seconds x attempts + backoff`, not `timeout_seconds`.
#
# The `cap`/`deploy` phases assert the shape of every attempt, never the retry
# count — the count is provider policy and may change, while "each attempt is
# bounded by the configured second" is the contract. Anyone reading the Panel's
# seconds field as "how long I will wait" is off by the retry factor; that is a
# labelling question for the Panel, deliberately not fixed in this round.
set -uo pipefail

PHASE="${1:-cap}"
case "$PHASE" in
  cap|auto|deploy|precedence|panel) ;;
  *) echo "unknown phase: $PHASE (cap|auto|deploy|precedence|panel)" >&2; exit 64 ;;
esac

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18831}"
MOCK_PORT="${MOCK_PORT:-18832}"
# Longer than any cap a phase configures and than any phase's watch window, so
# "the request ended" is always the server's doing and never the mock's.
STALL_MS="${STALL_MS:-60000}"

command -v node >/dev/null 2>&1 || { echo "node is required for this fixture" >&2; exit 1; }

# Sourced above the build block it serves — see qa/lib/build.sh.
. "$HERE/../lib/build.sh"

# --- build BEFORE the HOME redirect ----------------------------------------
# On Windows the pinned RUSTUP_HOME/CARGO_HOME are msys paths the native
# toolchain cannot read, so every cargo invocation happens before any redirect.
# Nothing after this block runs cargo.
if [ "${SKIP_BUILD:-0}" != "1" ]; then
  echo "=== build ==="
  qa_build --bin aleph-server || { echo "server build failed" >&2; exit 1; }
fi
TARGET_DIR="$(cd "$REPO" && cargo metadata --format-version 1 --no-deps 2>/dev/null \
  | node -e 'let s="";process.stdin.on("data",c=>s+=c).on("end",()=>console.log(JSON.parse(s).target_directory))')"
SERVER="$TARGET_DIR/debug/aleph-server"
[ -x "$SERVER" ] || SERVER="$SERVER.exe"
[ -x "$SERVER" ] || { echo "no server binary under $TARGET_DIR/debug" >&2; exit 1; }
# The one trace a swallowed build failure leaves behind.
echo "binary: $SERVER ($(date -r "$SERVER" '+%Y-%m-%d %H:%M:%S'))"

# Is this binary actually the code in the tree?
#
# On 2026-09-06 this fixture reported `cap` and `deploy` RED for two hours, with
# `/inflight` flat and no request ever cut, against a binary whose mtime was
# 00:01:36 while the source had not changed since 23:27. Rebuilding -- changing
# nothing else -- turned all four phases green. Every conclusion drawn in that
# window was about a binary, not about the code, and the fixture had printed the
# binary's timestamp all along without anyone able to say what it should be.
#
# `find -newer` answers the only question that matters: is there a source file
# younger than the thing under test? SKIP_BUILD=1 is the flag that makes this
# reachable, so this is exactly where it has to be checked -- the flag says
# "reuse what is there", and reusing something older than the code is the trap
# it opens (判据 §18 -- the instrument gets doubted first, and a binary IS the
# instrument here).
STALE="$(cd "$REPO" && find src shared crates -name '*.rs' -newer "$SERVER" -print -quit 2>/dev/null)"
[ -n "$STALE" ] || STALE="$(cd "$REPO" && find Cargo.toml Cargo.lock -newer "$SERVER" -print -quit 2>/dev/null)"
if [ -n "$STALE" ]; then
  echo "HARNESS_STALE_BINARY"
  echo "  $STALE is newer than the server binary."
  echo "  Re-run WITHOUT SKIP_BUILD=1. A stale binary does not fail loudly here;"
  echo "  it produces confident, wrong measurements."
  exit 1
fi

# --- scratch root ----------------------------------------------------------
# Mixed form on Windows (`C:/…`): the native server rejects the msys `/c/…`
# form and silently resolves it against the current drive root instead.
if [ -z "${QA_ROOT:-}" ]; then
  QA_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aleph-qa-gentimeout-XXXXXX")"
  command -v cygpath >/dev/null 2>&1 && QA_ROOT="$(cygpath -m "$QA_ROOT")"
fi

. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
export REAL_HOME
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
MOCK_LOG="$QA_ROOT/mock-requests.jsonl"
: > "$MOCK_LOG"
export RUST_MIN_STACK="${RUST_MIN_STACK:-268435456}"

SERVER_PID=""
MOCK_PID=""
GEN_PID=""
say() { printf '\n=== %s ===\n' "$*"; }

# `kill` from Git Bash does not reliably stop a NATIVE Windows child, and the
# failure is silent: the fixture prints its cleanup line and the process keeps
# running. Measured on 2026-09-06 -- `aleph-server.exe` processes started by
# this fixture were still listening minutes after the run that spawned them had
# exited and printed its cleanup (one of them, PID 3800, created 00:42:26, was
# found holding a port at 00:47). The next run then fails to bind, or -- worse --
# finds the port already answering and measures a process it does not control.
# `taskkill //F //T` is the spelling that actually works on a native child.
qa_kill() {
  local pid="$1"
  [ -n "$pid" ] || return 0
  if command -v taskkill >/dev/null 2>&1; then
    # //T takes the whole tree; the double slash is the msys escape.
    taskkill //PID "$pid" //F //T >/dev/null 2>&1 && return 0
  fi
  kill -9 "$pid" 2>/dev/null
}

cleanup() {
  for pid in "$SERVER_PID" "$MOCK_PID" "$GEN_PID"; do
    [ -n "$pid" ] && kill "$pid" 2>/dev/null
  done
  sleep 1
  for pid in "$SERVER_PID" "$MOCK_PID" "$GEN_PID"; do
    qa_kill "$pid"
  done
  if [ "$KEEP" = "1" ]; then echo "artifacts kept in $QA_ROOT"; else rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

say "generate a baseline config"
# `--port` on the generation boot: the config does not exist yet, so without it
# this boot binds the built-in default port, and anything already holding it
# makes the process exit before writing a config at all. The symptom reads like
# a path problem.
#
# No `--config` on THIS boot: pinning a path that does not exist yet makes the
# server exit with "Failed to read ... (os error 2)" instead of generating one.
# The pin goes on the second boot, which is the one that measures.
"$SERVER" --port "$GATEWAY_PORT" start >"$QA_ROOT/gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 60); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null; sleep 1; qa_kill "$GEN_PID"; wait "$GEN_PID" 2>/dev/null
GEN_PID=""
[ -f "$CONFIG" ] || { echo "no config generated at $CONFIG"; tail -30 "$QA_ROOT/gen.log"; exit 1; }

# Each phase is one point in the precedence chain the round rebuilt:
#   explicit config > ~/.aleph/defaults.toml > the provider's own default
say "patch config for phase: $PHASE"
case "$PHASE" in
  cap)
    node "$HERE/patch_config.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" 2 || exit 1
    ;;
  auto|panel)
    # No provider timeout, no defaults.toml: the state that used to be
    # impossible to express, because serde's default made it read as 120.
    node "$HERE/patch_config.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" none || exit 1
    ;;
  deploy)
    # The deployment-wide override, which this round MOVED out of
    # `#[serde(default = ...)]` and into `request_timeout_secs`. Moving a wire
    # is how a wire gets severed, and nothing else in the suite watches this one.
    node "$HERE/patch_config.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" none || exit 1
    printf '[generation]\ntimeout_seconds = 2\n' > "$ALEPH_HOME/defaults.toml"
    echo "wrote $ALEPH_HOME/defaults.toml with generation timeout_seconds = 2"
    ;;
  precedence)
    # Both set, and they disagree. Without this arm `deploy`'s green is also
    # consistent with "the override always wins", which is a different rule.
    node "$HERE/patch_config.mjs" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT" 30 || exit 1
    printf '[generation]\ntimeout_seconds = 2\n' > "$ALEPH_HOME/defaults.toml"
    echo "wrote defaults.toml = 2 against an explicit provider timeout of 30"
    ;;
esac

say "start the stalling provider"
node "$HERE/mock_stall.mjs" "$MOCK_PORT" "$STALL_MS" "$MOCK_LOG" >"$QA_ROOT/mock.log" 2>&1 &
MOCK_PID=$!
MOCK_UP=0
for _ in $(seq 1 40); do
  curl -sf -o /dev/null "http://127.0.0.1:$MOCK_PORT/health" 2>/dev/null && { MOCK_UP=1; break; }
  sleep 0.25
done
[ "$MOCK_UP" = "1" ] || { echo "HARNESS_MOCK_NEVER_CAME_UP"; tail -20 "$QA_ROOT/mock.log"; exit 1; }

say "start the server"
"$SERVER" --config "$CONFIG" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
UP=0
for _ in $(seq 1 120); do
  if curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null; then UP=1; break; fi
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "server died"; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
# "I could not ask" must never render as "the answer is no".
[ "$UP" = "1" ] || { echo "HARNESS_GATEWAY_NEVER_CAME_UP"; tail -40 "$QA_ROOT/server.log"; exit 1; }
echo "gateway up on $GATEWAY_PORT"

# The isolation this fixture claims, asserted rather than assumed.
#
# `qa_redirect_home` sets the environment; nothing downstream confirms the
# server agreed. This reads the server's OWN report of which file it watches,
# which is the only observation that can tell "isolated" from "quietly running
# against the operator's real ~/.aleph". It costs one grep and it is the guard
# that would catch the worst outcome this fixture can have.
#
# It earned its place the hard way: for two hours on 2026-09-06 a stale binary
# WAS reading `C:\Users\<you>\.aleph/config.toml` while every log line looked
# ordinary. That turned out to be the stale binary, not a defect in the server
# (the check below reproduces clean on a current build) -- but nothing in the
# fixture had been able to say so either way, and that is the gap being closed.
WATCHED="$(sed -n 's/^Hot config reload enabled: //p' "$QA_ROOT/server.log" | tail -1)"
case "$WATCHED" in
  "") echo "HARNESS_NO_CONFIG_PATH_REPORTED (cannot prove isolation)"; exit 1 ;;
esac
# Compare on the leaf the scratch root contributes; the server prints a mixed
# separator form (`.../home/.aleph\config.toml`) that no plain string equality
# against "$CONFIG" survives.
case "$WATCHED" in
  *"$(basename "$QA_ROOT")"*) echo "config isolation OK: $WATCHED" ;;
  *) echo "HARNESS_ESCAPED_THE_SCRATCH_HOME"
     echo "  the server is watching : $WATCHED"
     echo "  this fixture's config  : $CONFIG"
     echo "  Refusing to measure anything: a run that reads the operator's real"
     echo "  ~/.aleph is the one thing every fixture here promises not to do."
     exit 1 ;;
esac

# The provider has to have REGISTERED, or every phase measures the tool's
# "no provider" error instead of a timeout. This is a precondition, so it exits
# with a marker rather than failing an assertion.
#
# Read the `println!` in generation_init.rs, not the `tracing::info!` two lines
# above it. The first draft grepped for "Registered generation provider" — that
# string exists in the source, so the guard looked correct, but it is a tracing
# event that the default filter never emits. The guard was therefore red on a
# server that HAD registered the provider: a guard that only recognises the shape
# its author imagined (判据 §3), and one whose red meant nothing (判据 §2).
#
# `println!("  Generation providers: {} registered", registry.len())` is guarded
# by `!registry.is_empty() && !daemon`, and this fixture never starts a daemon —
# so an absent line really does mean "none registered", and the count is on it.
REGISTERED="$(sed -n 's/^ *Generation providers: \([0-9]\+\) registered$/\1/p' \
  "$QA_ROOT/server.log" 2>/dev/null | tail -1)"
if [ -z "$REGISTERED" ]; then
  echo "HARNESS_PROVIDER_NOT_REGISTERED (no count line in the boot output)"
  grep -i "generation\|provider" "$QA_ROOT/server.log" | tail -20
  exit 1
fi
# Exactly one: patch_config.mjs deletes the whole [generation] section and
# appends a single provider. Any other number means the phase is about to time
# a provider this fixture did not configure, which is worth a loud stop rather
# than a quiet measurement of the wrong thing.
if [ "$REGISTERED" != "1" ]; then
  echo "HARNESS_UNEXPECTED_PROVIDER_COUNT=$REGISTERED (expected exactly qa-stall)"
  grep -i "generation" "$QA_ROOT/server.log" | tail -20
  exit 1
fi
echo "generation providers registered: $REGISTERED (qa-stall)"

RC=0
case "$PHASE" in
  panel)
    cat <<EOF

=== panel scenario: holding ===
  Panel:       http://127.0.0.1:$GATEWAY_PORT/settings/generation-providers
               (the arm that renders it: interfaces/webchat/src/app.rs's
                desktop_settings_body, "/settings/generation-providers" =>
                GenerationProvidersView. \`/dashboard/*\` and \`/settings/*\`
                are SIBLING route families, not a prefix and a subpath — the
                first draft of this line spelled a URL that renders nothing,
                which would have read as "the Panel is broken".)
  provider:    qa-stall (image), configured with NO timeout_seconds
  config file: $CONFIG
  server log:  $QA_ROOT/server.log

  What to check in the browser, in this order:
    1. open qa-stall's detail panel. The timeout row must read "Auto" and the
       Auto checkbox must be CHECKED, with the number input disabled — the
       config has no timeout_seconds, and a slider showing 60 would be the
       Panel inventing a value the server never sent.
    2. uncheck Auto, set a number, Save. \`grep timeout_seconds "$CONFIG"\`
       must now show it under [generation.image_providers.qa-stall].
    3. check Auto again, Save. The key must be GONE from the config file —
       not written as 0, not left at the old number. That is the assertion
       \`just wasm\` cannot make: it proves the field is omitted from the
       payload, which is the only way "unset" crosses the wire.
    4. reload the page and reopen the panel: it must come back Auto.

  stop: touch "$QA_ROOT/stop"   (or Ctrl-C)
EOF
    while [ ! -f "$QA_ROOT/stop" ]; do sleep 1; done
    ;;
  *)
    say "drive ($PHASE)"
    node "$HERE/drive_timeout.mjs" "$GATEWAY_PORT" "$MOCK_PORT" "$MOCK_LOG" "$PHASE"
    RC=$?
    if [ "$RC" != "0" ]; then
      echo; echo "--- server log tail ---"; tail -40 "$QA_ROOT/server.log"
      echo "--- mock log tail ---"; tail -30 "$QA_ROOT/mock.log"
    fi
    ;;
esac

exit "$RC"
