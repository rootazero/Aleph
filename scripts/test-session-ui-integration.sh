#!/usr/bin/env bash
set -euo pipefail

# End-to-end checks for the Panel session-management RPC surface.
# The Gateway RPC transport is WebSocket; use the project's CLI client rather
# than POSTing JSON to /ws as if it were an HTTP endpoint.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SERVER_BIN="${ALEPH_SERVER_BIN:-$PROJECT_ROOT/target/debug/aleph-server}"
GATEWAY_URL="${ALEPH_GATEWAY_URL:-ws://127.0.0.1:18790/ws}"
GATEWAY_HTTP_URL="${ALEPH_HTTP_URL:-http://127.0.0.1:18790}"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

if [[ ! -x "$SERVER_BIN" ]]; then
    SERVER_BIN="${ALEPH_SERVER_BIN:-aleph-server}"
fi

rpc_call() {
    local method="$1"
    local params="$2"
    "$SERVER_BIN" gateway call "$method" \
        --url "$GATEWAY_URL" \
        --timeout 30000 \
        --params "$params"
}

check_server() {
    echo -n "Checking if server is running... "
    if curl -fsS --max-time 10 "$GATEWAY_HTTP_URL/health" >/dev/null 2>&1; then
        echo -e "${GREEN}✓${NC}"
    else
        echo -e "${RED}✗${NC}"
        echo "ERROR: Server not running or health endpoint unavailable."
        return 1
    fi
}

check_handlers() {
    echo -n "Checking if session management handlers are registered... "
    local response
    if response="$(rpc_call memory.sessions.list '{"limit":1,"offset":0}')" \
        && echo "$response" | jq -e '.sessions' >/dev/null; then
        echo -e "${GREEN}✓${NC}"
    else
        echo -e "${RED}✗${NC}"
        echo "Unexpected response: ${response:-<empty>}"
        return 1
    fi
}

create_test_data() {
    echo
    echo "=== Existing Test Session Data ==="
    local response total
    response="$(rpc_call memory.sessions.list '{"limit":100,"offset":0}')"
    total="$(echo "$response" | jq -r '.total // 0')"
    echo "Found $total existing sessions"
    if [[ "$total" -lt 1 ]]; then
        echo -e "${YELLOW}WARNING: No sessions found. Detail and attachment checks will skip.${NC}"
    fi
}

test_session_list() {
    echo
    echo "=== Test 1: Session List ==="
    local response
    response="$(rpc_call memory.sessions.list '{"limit":20,"offset":0}')"
    if echo "$response" | jq -e '.sessions' >/dev/null; then
        echo -e "${GREEN}✓${NC} List returned $(echo "$response" | jq -r '.total') sessions"
    else
        echo -e "${RED}✗${NC} List failed: $response${NC}"
        return 1
    fi
}

test_session_detail() {
    echo
    echo "=== Test 2: Session Detail ==="
    local list session_id response
    list="$(rpc_call memory.sessions.list '{"limit":1,"offset":0}')"
    session_id="$(echo "$list" | jq -r '.sessions[0].session_id // empty')"
    if [[ -z "$session_id" ]]; then
        echo -e "${YELLOW}⊘${NC} No sessions to test"
        return 0
    fi

    response="$(rpc_call memory.sessions.get "{\"session_id\":$(jq -Rn --arg id "$session_id" '$id')}")"
    if echo "$response" | jq -e '.metadata' >/dev/null; then
        echo -e "${GREEN}✓${NC} Detail loaded: transcript=$(echo "$response" | jq -r '.transcript | length') bytes"
    else
        echo -e "${RED}✗${NC} Detail failed: $response"
        return 1
    fi
}

test_search() {
    echo
    echo "=== Test 3: Full-Text Search ==="
    local response
    response="$(rpc_call memory.sessions.search '{"query":"test","limit":10}')"
    if echo "$response" | jq -e '.results' >/dev/null; then
        echo -e "${GREEN}✓${NC} Search returned $(echo "$response" | jq -r '.results | length') results"
    else
        echo -e "${RED}✗${NC} Search failed: $response"
        return 1
    fi
}

test_filters() {
    echo
    echo "=== Test 4: Filters ==="
    local response
    response="$(rpc_call memory.sessions.list '{"agent_id":"test-agent","limit":10,"offset":0}')"
    if echo "$response" | jq -e '.sessions' >/dev/null; then
        echo -e "${GREEN}✓${NC} Agent filter works"
    else
        echo -e "${RED}✗${NC} Agent filter failed: $response"
        return 1
    fi
}

test_attachment_download() {
    echo
    echo "=== Test 5: Attachment Download ==="
    local list session_id detail attachment_id http_code
    list="$(rpc_call memory.sessions.list '{"limit":100,"offset":0}')"
    session_id=""
    while IFS= read -r candidate; do
        [[ -z "$candidate" ]] && continue
        detail="$(rpc_call memory.sessions.get "{\"session_id\":$(jq -Rn --arg id "$candidate" '$id')}")"
        attachment_id="$(echo "$detail" | jq -r '.attachments[0].attachment_id // empty')"
        if [[ -n "$attachment_id" ]]; then
            session_id="$candidate"
            break
        fi
    done < <(echo "$list" | jq -r '.sessions[]?.session_id')

    if [[ -z "$session_id" ]]; then
        echo -e "${YELLOW}⊘${NC} No sessions with attachments to test"
        return 0
    fi

    http_code="$(curl -sS -o /dev/null -w '%{http_code}' \
        "http://127.0.0.1:18790/api/memory/sessions/attachment/${session_id}:${attachment_id}")"
    if [[ "$http_code" == "200" ]]; then
        echo -e "${GREEN}✓${NC} Attachment download works (HTTP $http_code)"
    else
        echo -e "${RED}✗${NC} Attachment download failed (HTTP $http_code)"
        return 1
    fi
}

test_performance() {
    echo
    echo "=== Test 6: Performance ==="
    local start end latency response
    start="$(python3 -c 'import time; print(time.monotonic_ns())')"
    response="$(rpc_call memory.sessions.list '{"limit":20,"offset":0}')"
    end="$(python3 -c 'import time; print(time.monotonic_ns())')"
    latency="$(( (end - start) / 1000000 ))"
    if echo "$response" | jq -e '.sessions' >/dev/null && [[ "$latency" -lt 500 ]]; then
        echo -e "${GREEN}✓${NC} List RPC latency: ${latency}ms (< 500ms target)"
    elif echo "$response" | jq -e '.sessions' >/dev/null; then
        echo -e "${YELLOW}⚠${NC} List RPC latency: ${latency}ms (> 500ms target)"
    else
        echo -e "${RED}✗${NC} Performance RPC failed: $response"
        return 1
    fi
}

main() {
    check_server
    check_handlers
    create_test_data

    local passed=0 failed=0
    if test_session_list; then ((passed += 1)); else ((failed += 1)); fi
    if test_session_detail; then ((passed += 1)); else ((failed += 1)); fi
    if test_search; then ((passed += 1)); else ((failed += 1)); fi
    if test_filters; then ((passed += 1)); else ((failed += 1)); fi
    if test_attachment_download; then ((passed += 1)); else ((failed += 1)); fi
    if test_performance; then ((passed += 1)); else ((failed += 1)); fi

    echo
    echo "=== Test Summary ==="
    echo -e "Passed: ${GREEN}$passed${NC}"
    echo -e "Failed: ${RED}$failed${NC}"
    [[ "$failed" -eq 0 ]]
}

main "$@"
