#!/usr/bin/env bash
# Pinned official A2A Python SDK inbound QA runner.
# Usage: ./qa/a2a/run.sh check | gateway 0.3.26|1.0.3
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
MODE="${1:-check}"
SDK_VERSION="${2:-1.0.3}"
case "$SDK_VERSION" in
  0.3.26) SDK_TAG=v0_3_26 ;;
  1.0.3) SDK_TAG=v1_0_3 ;;
  *) echo "supported SDK versions: 0.3.26, 1.0.3" >&2; exit 64 ;;
esac
command -v uv >/dev/null || { echo "uv is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }

if [ "$MODE" = check ]; then
  OWN_QA_ROOT=0
  if [ -z "${QA_ROOT:-}" ]; then
    QA_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aleph-a2a-sdk-XXXXXX")"
    OWN_QA_ROOT=1
  else
    mkdir -p "$QA_ROOT"
  fi
  mkdir -p "$QA_ROOT"
  trap '[ "$OWN_QA_ROOT" != 1 ] || [ "${KEEP:-0}" = 1 ] || rm -rf "$QA_ROOT"' EXIT
  ENV_DIR="$QA_ROOT/venv-$SDK_TAG"
  export UV_CACHE_DIR="$QA_ROOT/uv-cache"
  uv venv "$ENV_DIR" --python "$(command -v python3)"
  uv pip install --python "$ENV_DIR/bin/python" "a2a-sdk==$SDK_VERSION"
  "$ENV_DIR/bin/python" "$HERE/sdk_probe.py" "$SDK_VERSION"
  echo "PASS: a2a-sdk==$SDK_VERSION isolated environment and public client API"
  exit 0
fi

if [ "$MODE" != gateway ]; then
  echo "usage: $0 check|gateway [0.3.26|1.0.3]" >&2
  exit 64
fi

# The caller supplies an already-built server; this runner only exercises it.
SERVER_BIN="${ALEPH_SERVER_BIN:-}"
[ -x "$SERVER_BIN" ] || { echo "set ALEPH_SERVER_BIN to an executable aleph-server; no build is run" >&2; exit 2; }
OWN_QA_ROOT=0
if [ -z "${QA_ROOT:-}" ]; then
  QA_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aleph-a2a-sdk-XXXXXX")"
  OWN_QA_ROOT=1
else
  mkdir -p "$QA_ROOT"
fi
KEEP="${KEEP:-0}"
GATEWAY_PORT="${GATEWAY_PORT:-18796}"
MOCK_PORT="${MOCK_PORT:-18996}"
. "$HERE/../lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
mkdir -p "$ALEPH_HOME"
CONFIG="$ALEPH_HOME/config.toml"
MOCK_PID=""
SERVER_PID=""
cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
  [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null || true
  [ -n "$SERVER_PID" ] && wait "$SERVER_PID" 2>/dev/null || true
  [ -n "$MOCK_PID" ] && wait "$MOCK_PID" 2>/dev/null || true
  if [ "$KEEP" = 1 ]; then echo "scratch artifacts kept at $QA_ROOT"; elif [ "$OWN_QA_ROOT" = 1 ]; then rm -rf "$QA_ROOT"; fi
}
trap cleanup EXIT

# First boot creates the schema-compatible default config in the isolated home.
timeout 25 "$SERVER_BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/config-gen.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 50); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null || true
wait "$GEN_PID" 2>/dev/null || true
[ -f "$CONFIG" ] || { echo "server did not generate $CONFIG" >&2; tail -30 "$QA_ROOT/config-gen.log"; exit 1; }
python3 "$HERE/enable_a2a.py" "$CONFIG" "$GATEWAY_PORT" "$MOCK_PORT"
python3 "$REPO/qa/busy_input/mock_anthropic.py" "$MOCK_PORT" /etc/hostname a2a >"$QA_ROOT/mock-provider.log" 2>&1 &
MOCK_PID=$!
"$SERVER_BIN" start >"$QA_ROOT/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 1 90); do
  if curl -fsS "http://127.0.0.1:$GATEWAY_PORT/health" >/dev/null 2>&1; then break; fi
  kill -0 "$SERVER_PID" 2>/dev/null || { echo "Gateway exited during startup" >&2; tail -40 "$QA_ROOT/server.log"; exit 1; }
  sleep 1
done
curl -fsS "http://127.0.0.1:$GATEWAY_PORT/health" >/dev/null || {
  echo "Gateway did not become ready" >&2
  tail -40 "$QA_ROOT/server.log"
  exit 1
}

ENV_DIR="$QA_ROOT/venv-$SDK_TAG"
export UV_CACHE_DIR="$QA_ROOT/uv-cache"
uv venv "$ENV_DIR" --python "$(command -v python3)"
uv pip install --python "$ENV_DIR/bin/python" "a2a-sdk==$SDK_VERSION"
"$ENV_DIR/bin/python" "$HERE/sdk_probe.py" "$SDK_VERSION" "http://127.0.0.1:$GATEWAY_PORT" "$QA_ROOT"
