#!/usr/bin/env bash
# Real-current-binary diagnostics supplement QA. Exit 3 is intentionally not PASS.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
GUARD="$REPO/.superpowers/sdd/2026-10-07-capability-phase4-h-pre-runtime-mount/cargo-guard.py"
PATCH="$REPO/qa/busy_input/patch_config.py"
PYTHON="${PYTHON:-$(command -v python3)}"
GATEWAY_PORT="${GATEWAY_PORT:-18831}"
MOCK_PORT="${MOCK_PORT:-18832}"
KEEP="${KEEP:-0}"
# Debug aid only: the evidence run uses the default (every scenario and tls).
SCENARIOS="${SCENARIOS:-disabled initial replacement ownership delivery source close negative tls}"
want() { case " $SCENARIOS " in *" $1 "*) return 0;; esac; return 1; }
QA_ROOT="${QA_ROOT:-$(mktemp -d /tmp/aleph-qa-hpre-XXXXXX)}"
SERVER_PID=""
MOCK_PID=""

cleanup() {
    [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
    [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null || true
    [ -n "$SERVER_PID" ] && wait "$SERVER_PID" 2>/dev/null || true
    [ -n "$MOCK_PID" ] && wait "$MOCK_PID" 2>/dev/null || true
    if [ "$KEEP" != 1 ]; then rm -rf "$QA_ROOT"; else printf 'QA_ROOT kept: %s\n' "$QA_ROOT"; fi
}
trap cleanup EXIT INT TERM

mkdir -p "$QA_ROOT"
HEAD_BEFORE="$(git -C "$REPO" rev-parse HEAD)"
BUILD_STARTED="$(date +%s)"
printf 'HEAD_BEFORE: %s\n' "$HEAD_BEFORE"
printf 'BUILD_COMMAND: %s build --bin aleph-server\n' "$GUARD"
# This is deliberately before HOME redirection and is the only Cargo entry point.
"$PYTHON" "$GUARD" build --bin aleph-server >"$QA_ROOT/build.log" 2>&1
BUILD_RC=$?
cat "$QA_ROOT/build.log"
[ "$BUILD_RC" -eq 0 ] || { echo "BUILD_RESULT: FAIL exit=$BUILD_RC"; exit "$BUILD_RC"; }
HEAD_AFTER="$(git -C "$REPO" rev-parse HEAD)"
[ "$HEAD_AFTER" = "$HEAD_BEFORE" ] || { echo "STALE_GUARD: HEAD changed during build"; exit 1; }
BIN="$REPO/target/debug/aleph-server"
[ -x "$BIN" ] || { echo "STALE_GUARD: no built aleph-server at $BIN"; exit 1; }
BIN_MTIME="$(stat -f %m "$BIN")"
if [ "$BIN_MTIME" -lt "$BUILD_STARTED" ]; then
    grep -Eq 'Fresh (aleph-server|aleph-server v)|Finished .*target' "$QA_ROOT/build.log" || {
        echo "STALE_GUARD: binary predates this build without a Cargo Fresh record"; exit 1;
    }
    printf 'BINARY_MTIME_GUARD: Fresh artifact (mtime=%s build_started=%s)\n' "$BIN_MTIME" "$BUILD_STARTED"
else
    printf 'BINARY_MTIME_GUARD: rebuilt artifact (mtime=%s build_started=%s)\n' "$BIN_MTIME" "$BUILD_STARTED"
fi
BIN_SHA="$(shasum -a 256 "$BIN" | awk '{print $1}')"
printf 'BINARY: %s\nBINARY_SHA256: %s\n' "$BIN" "$BIN_SHA"

. "$REPO/qa/lib/scratch_home.sh"
qa_redirect_home "$QA_ROOT"
export REAL_HOME
CONFIG="$ALEPH_HOME/config.toml"
"$BIN" --port "$GATEWAY_PORT" start >"$QA_ROOT/generate.log" 2>&1 &
GEN_PID=$!
for _ in $(seq 1 60); do [ -f "$CONFIG" ] && break; sleep 0.5; done
kill "$GEN_PID" 2>/dev/null || true
wait "$GEN_PID" 2>/dev/null || true
[ -f "$CONFIG" ] || { cat "$QA_ROOT/generate.log"; echo "SETUP_RESULT: FAIL no config"; exit 1; }
"$PYTHON" "$PATCH" "$CONFIG" --gateway-port "$GATEWAY_PORT" --mock-port "$MOCK_PORT" || exit 1
# Expose the diagnostic tool (plus a read-only builtin control) on Aleph's own
# loopback `/mcp` face, so the disabled arm's MCP `tools/list` absence is
# decided by the runtime registry, not by the default expose whitelist.
"$PYTHON" "$REPO/qa/mcp_face/patch_mcp.py" "$CONFIG" --expose "capability_projection_diagnostics,grep" || exit 1

# Runtime-only MCP fixture. The MCP mutation itself remains the existing
# mcp_config.create/update/delete route; this process supplies a real handler
# effect receipt rather than using a catalog or library counter as evidence.
FIXTURE_SCRIPT="$QA_ROOT/effect_mcp.py"
"$PYTHON" - "$FIXTURE_SCRIPT" <<'PY'
import sys
from pathlib import Path
Path(sys.argv[1]).write_text(r'''import json, sys
log = sys.argv[1]
try:
    temporary_count = int(sys.argv[2]) if len(sys.argv) > 2 else 1
except (TypeError, ValueError):
    raise SystemExit("temporary tool count must be an integer")
if temporary_count < 0:
    raise SystemExit("temporary tool count must be non-negative")

def send(x):
    sys.stdout.write(json.dumps(x) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    try: req = json.loads(line)
    except Exception: continue
    method, rid, params = req.get("method"), req.get("id"), req.get("params", {})
    if method == "initialize":
        send({"jsonrpc":"2.0","id":rid,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"qa-effect","version":"1"}}})
    elif method == "notifications/initialized": pass
    elif method == "tools/list":
        tools = [{"name":"qa_echo","description":"QA effect sink","inputSchema":{"type":"object"}}]
        tools.extend({"name":f"temporaryqa_echo_{i}","description":"temporary QA capacity fixture","inputSchema":{"type":"object"}} for i in range(temporary_count))
        send({"jsonrpc":"2.0","id":rid,"result":{"tools":tools}})
    elif method == "tools/call":
        args = params.get("arguments", {})
        with open(log, "a") as fh: fh.write(json.dumps({"method":method,"name":params.get("name"),"arguments":args})+"\n")
        send({"jsonrpc":"2.0","id":rid,"result":{"content":[{"type":"text","text":"QA_EFFECT_RECEIPT:"+json.dumps(args,sort_keys=True)}],"isError":False}})
    elif method == "ping": send({"jsonrpc":"2.0","id":rid,"result":{}})
    elif rid is not None: send({"jsonrpc":"2.0","id":rid,"error":{"code":-32601,"message":"method not found"}})
''')
PY

WS="ws://127.0.0.1:$GATEWAY_PORT/ws"
RUN_ID="$(printf '%s' "$HEAD_BEFORE:$BIN_SHA:$$" | shasum -a 256 | awk '{print substr($1,1,16)}')"
PREFIX="qa_hpre_${RUN_ID}"
RC=0

stop_children() {
    [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null || true
    [ -n "$MOCK_PID" ] && kill "$MOCK_PID" 2>/dev/null || true
    [ -n "$SERVER_PID" ] && wait "$SERVER_PID" 2>/dev/null || true
    [ -n "$MOCK_PID" ] && wait "$MOCK_PID" 2>/dev/null || true
    SERVER_PID=""; MOCK_PID=""
}

start_server() {
    local enabled="$1" log="$2"
    if [ "$enabled" = 1 ]; then
        env ALEPH_CAPABILITY_DIAGNOSTICS=1 ALEPH_GATEWAY_TOOLS_ALLOW=capability_projection_diagnostics \
            bash -c "cd '$QA_ROOT' && exec '$BIN' start" >"$log" 2>&1 &
    else
        env -u ALEPH_CAPABILITY_DIAGNOSTICS -u ALEPH_GATEWAY_TOOLS_ALLOW \
            bash -c "cd '$QA_ROOT' && exec '$BIN' start" >"$log" 2>&1 &
    fi
    SERVER_PID=$!
    for _ in $(seq 1 90); do
        curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null && break
        kill -0 "$SERVER_PID" 2>/dev/null || { cat "$log"; return 1; }
        sleep 1
    done
    curl -sf -o /dev/null "http://127.0.0.1:$GATEWAY_PORT/health" 2>/dev/null || { cat "$log"; return 1; }
    grep 'Mode:' "$log" | tail -1 || true
    grep -q 'Real AgentLoop' "$log" || { echo 'REAL_MODE_GUARD: FAIL'; cat "$log"; return 1; }
}

start_mock() {
    local spec="$1" log="$2" requests="$3"
    "$PYTHON" "$REPO/qa/busy_input/mock_anthropic.py" "$MOCK_PORT" /etc/hostname tool-chain "$spec" "$requests" >"$log" 2>&1 &
    MOCK_PID=$!
    for _ in $(seq 1 40); do
        curl -sf -o /dev/null "http://127.0.0.1:$MOCK_PORT/v1/models" 2>/dev/null && return 0
        sleep 0.5
    done
    cat "$log"; return 1
}

run_scenario() {
    local scenario="$1" spec_json="$2" enabled=1
    want "$scenario" || return 0
    local slog="$QA_ROOT/${scenario}.server.log" mlog="$QA_ROOT/${scenario}.mock.log"
    local requests="$QA_ROOT/${scenario}.requests.jsonl" effects="$QA_ROOT/${scenario}.effects.jsonl" spec="$QA_ROOT/${scenario}.spec.json" out="$QA_ROOT/${scenario}.qa.log"
    printf '%s' "$spec_json" >"$spec"
    : >"$requests"; : >"$effects"
    [ "$scenario" = disabled ] && enabled=0
    stop_children
    start_server "$enabled" "$slog" || { echo "SCENARIO_RESULT[$scenario]: exit=1"; RC=1; return; }
    # Every arm, including disabled, records the real provider request so the
    # disabled arm can assert tools[] absence on an actual AgentLoop request.
    start_mock "$spec" "$mlog" "$requests" || { echo "SCENARIO_RESULT[$scenario]: exit=2"; RC=2; stop_children; return; }
    echo "SCENARIO_COMMAND[$scenario]: $PYTHON $HERE/drive.py --scenario $scenario --ws $WS"
    "$PYTHON" "$HERE/drive.py" --scenario "$scenario" --ws "$WS" --provider-log "$requests" --effect-log "$effects" --fixture-script "$FIXTURE_SCRIPT" --python "$PYTHON" --prefix "$PREFIX" --run-id "$RUN_ID" >"$out" 2>&1
    local rc=$?; cat "$out"; echo "SCENARIO_RESULT[$scenario]: exit=$rc"
    [ "$rc" -ne 0 ] && RC="$rc"
    stop_children
}

run_scenario disabled '{"name":"capability_projection_diagnostics","input":{"operation":"status"}}'
run_scenario initial "{\"name\":\"${PREFIX}_initial__qa_echo\",\"input\":{\"marker\":\"QA_HPRE_INITIAL_${RUN_ID}\"}}"
run_scenario replacement "[{\"name\":\"${PREFIX}_replace_old__qa_echo\",\"input\":{\"marker\":\"QA_HPRE_REPLACEMENT_${RUN_ID}\"}},{\"name\":\"${PREFIX}_replace_new__qa_echo\",\"input\":{\"marker\":\"QA_HPRE_REPLACEMENT_${RUN_ID}\"}}]"
run_scenario ownership "{\"name\":\"${PREFIX}_owner__qa_echo\",\"input\":{\"marker\":\"QA_HPRE_OWNER_${RUN_ID}\"}}"
run_scenario delivery "{\"name\":\"${PREFIX}_delivery__qa_echo\",\"input\":{\"marker\":\"QA_HPRE_DELIVERY_${RUN_ID}\"}}"
run_scenario source "{\"name\":\"${PREFIX}_source__qa_echo\",\"input\":{\"marker\":\"QA_HPRE_SOURCE_${RUN_ID}\"}}"
run_scenario close "{\"name\":\"${PREFIX}_close__qa_echo\",\"input\":{\"marker\":\"QA_HPRE_CLOSE_${RUN_ID}\"}}"
run_scenario negative '{}'

# ---------------------------------------------------------------------------
# TLS identity phase: ONE explicitly named, really assigned LAN address; the
# disposable profile, port and certificate die with this run.  Without
# TLS_LAN_IP the phase is UNVERIFIED (exit 3) rather than silently skipped.
# ---------------------------------------------------------------------------
tls_phase() {
    want tls || return 0
    local ip="${TLS_LAN_IP:-}" out="$QA_ROOT/tls.qa.log" state="$QA_ROOT/tls.state.json"
    local requests="$QA_ROOT/tls.requests.jsonl" effects="$QA_ROOT/tls.effects.jsonl" spec="$QA_ROOT/tls.spec.json"
    local cafile="$ALEPH_HOME/data/tls/cert.pem" rc=0
    if [ -z "$ip" ]; then
        echo "TLS_PHASE: UNVERIFIED TLS_LAN_IP is not set (no LAN identity claim was made)"
        [ "$RC" -eq 0 ] && RC=3; return
    fi
    "$PYTHON" "$HERE/tls_negative.py" check-ip "$ip" || { echo "TLS_PHASE: FAIL LAN address guard"; RC=1; return; }
    if lsof -nP -iTCP:"$GATEWAY_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
        echo "TLS_PHASE: FAIL port $GATEWAY_PORT already has a listener"; RC=1; return
    fi
    : >"$requests"; : >"$effects"
    printf '%s' '{"name":"capability_projection_diagnostics","input":{"operation":"bump_runtime"}}' >"$spec"
    # 1. loopback preparation: users and user-bound tickets, plain loopback.
    stop_children
    start_server 1 "$QA_ROOT/tls.prepare.server.log" || { echo "TLS_PHASE: FAIL prepare server"; RC=1; return; }
    "$PYTHON" "$HERE/tls_negative.py" prepare --ws "$WS" --state "$state" --run-id "$RUN_ID" >"$out" 2>&1
    rc=$?; cat "$out"; stop_children
    [ "$rc" -ne 0 ] && { echo "TLS_PHASE: FAIL preparation exit=$rc"; RC=1; return; }
    # 2. switch the disposable profile to native TLS on the single LAN address.
    cp "$CONFIG" "$CONFIG.pre-tls"
    "$PYTHON" "$HERE/tls_negative.py" patch-config --config "$CONFIG" --host "$ip" || { RC=1; return; }
    env ALEPH_CAPABILITY_DIAGNOSTICS=1 ALEPH_GATEWAY_TOOLS_ALLOW=capability_projection_diagnostics \
        bash -c "cd '$QA_ROOT' && exec '$BIN' start" >"$QA_ROOT/tls.server.log" 2>&1 &
    SERVER_PID=$!
    local up=0
    for _ in $(seq 1 90); do
        curl -sf -o /dev/null --cacert "$cafile" "https://$ip:$GATEWAY_PORT/health" 2>/dev/null && { up=1; break; }
        kill -0 "$SERVER_PID" 2>/dev/null || break
        sleep 1
    done
    if [ "$up" -ne 1 ]; then echo "TLS_PHASE: FAIL TLS server did not become healthy"; tail -40 "$QA_ROOT/tls.server.log"; stop_children; RC=1; return; fi
    printf 'TLS_LISTENER: %s\n' "$(lsof -nP -iTCP:"$GATEWAY_PORT" -sTCP:LISTEN 2>/dev/null | awk 'NR>1{print $9}' | sort -u | tr '\n' ' ')"
    start_mock "$spec" "$QA_ROOT/tls.mock.log" "$requests" || { echo "TLS_PHASE: FAIL mock"; stop_children; RC=2; return; }
    "$PYTHON" "$HERE/tls_negative.py" probe --host "$ip" --port "$GATEWAY_PORT" --cafile "$cafile" --state "$state" \
        --provider-log "$requests" --effect-log "$effects" --fixture-script "$FIXTURE_SCRIPT" --python "$PYTHON" \
        --prefix "$PREFIX" --run-id "$RUN_ID" >"$out" 2>&1
    rc=$?; cat "$out"; echo "SCENARIO_RESULT[tls]: exit=$rc"
    stop_children
    [ "$rc" -ne 0 ] && RC="$rc"
    if lsof -nP -iTCP:"$GATEWAY_PORT" -sTCP:LISTEN >/dev/null 2>&1; then
        echo "TLS_CLEANUP: FAIL listener remains on $GATEWAY_PORT"; RC=1
    else
        echo "TLS_CLEANUP: OK no listener on $GATEWAY_PORT"
    fi
    cp "$CONFIG.pre-tls" "$CONFIG"
}

tls_phase

printf 'QA_ROOT: %s\nOVERALL_RESULT: exit=%s\n' "$QA_ROOT" "$RC"
exit "$RC"
