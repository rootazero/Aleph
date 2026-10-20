# Phase D: UI Integration Test - Session Management

**Document Version**: 1.0
**Test Date**: 2026-10-19
**Status**: ⚠️ **BLOCKED** - RPC handlers not wired during Gateway startup

---

## Executive Summary

The session management UI components (Tasks #20-24) are **code-complete** but **NOT FUNCTIONAL** because the RPC handlers are not registered during Gateway boot. This document provides:

1. **Current blocker analysis**
2. **Integration checklist** (manual test plan)
3. **Automated test script** (ready when handlers are wired)
4. **Expected vs actual behavior**

---

## 🚫 BLOCKER: Handlers Not Wired

### Issue

`register_session_management_handlers()` exists in `src/bin/aleph-server/commands/start/builder/handlers/memory.rs:470-496` but is **never called** during server startup.

### Evidence

```bash
# Handler registration function exists
$ grep -n "register_session_management_handlers" src/
src/bin/aleph-server/commands/start/builder/handlers/memory.rs:470

# But it's never called during boot
$ grep -rn "register_session_management_handlers" src/bin/aleph-server/commands/start/mod.rs
(no matches)
```

### Root Cause

The `MarkdownSessionStore` is instantiated at line 546-550 of `src/bin/aleph-server/commands/start/mod.rs`:

```rust
let markdown_store_result: Result<Arc<MarkdownSessionStore>, _> = async {
    let aleph_home = alephcore::utils::paths::get_config_dir()?;
    let store = MarkdownSessionStore::new(&aleph_home).await?;
    Ok(Arc::new(store))
}.await;
```

But it's only used to create a `MarkdownProjector` (line 551-565), **not** to register RPC handlers.

### Fix Required

In `src/bin/aleph-server/commands/start/mod.rs`, after the `MarkdownProjector` is created (around line 565), add:

```rust
// Register session management RPC handlers if store initialized successfully
if let Ok(ref markdown_store) = markdown_store_result {
    builder::handlers::memory::register_session_management_handlers(
        &mut gateway_server,
        markdown_store,
        daemon_mode,
    );
}
```

Also need to mount the attachment HTTP route in `src/gateway/server/mod.rs` (see Task #20 notes).

---

## Manual Integration Test Checklist

### Prerequisites

- [ ] **FIX APPLIED**: Handlers wired during Gateway boot (see above)
- [ ] Aleph server running (`aleph-server start`)
- [ ] Panel UI accessible at `http://localhost:18790`
- [ ] At least 3-5 existing sessions with varied data

### Test Scenario 1: Session List View

**Path**: Panel UI → Settings → Sessions

| Step | Action | Expected Result | Status |
|------|--------|----------------|--------|
| 1.1 | Navigate to Sessions tab | Grid layout loads: 60% list / 40% detail | ⬜ |
| 1.2 | Verify session cards display | Each card shows: agent icon, title, timestamp, duration, message count | ⬜ |
| 1.3 | Check loading state | Spinner visible during RPC call | ⬜ |
| 1.4 | Verify empty state | "No sessions found" message when no sessions exist | ⬜ |

### Test Scenario 2: Filters

| Step | Action | Expected Result | Status |
|------|--------|----------------|--------|
| 2.1 | Select agent filter | Only sessions from that agent shown | ⬜ |
| 2.2 | Select workspace filter | Only sessions from that workspace shown | ⬜ |
| 2.3 | Set date range | Only sessions within range shown | ⬜ |
| 2.4 | Select compression status | Only sessions with that status shown | ⬜ |
| 2.5 | Apply multiple filters | Combined filter logic works (AND) | ⬜ |
| 2.6 | Clear filters | All sessions shown again | ⬜ |

### Test Scenario 3: Pagination

| Step | Action | Expected Result | Status |
|------|--------|----------------|--------|
| 3.1 | Click "Next" button | Next page of sessions loads | ⬜ |
| 3.2 | Click "Previous" button | Previous page loads | ⬜ |
| 3.3 | Verify "has_more" indicator | "Next" disabled when no more results | ⬜ |
| 3.4 | Verify page state | Current page number displayed | ⬜ |

### Test Scenario 4: Session Detail - Transcript Tab

| Step | Action | Expected Result | Status |
|------|--------|----------------|--------|
| 4.1 | Click a session card | Detail view loads on right panel | ⬜ |
| 4.2 | Verify Transcript tab active | Blue underline on "Transcript" | ⬜ |
| 4.3 | Check message rendering | Messages display with role styling (User=blue, Assistant=green, System=yellow) | ⬜ |
| 4.4 | Verify markdown rendering | Code blocks, tables, lists render correctly | ⬜ |
| 4.5 | Check timestamps | Each message shows formatted timestamp | ⬜ |
| 4.6 | Scroll long transcripts | Scrollbar works, no layout breaks | ⬜ |

### Test Scenario 5: Session Detail - Tool Outputs Tab

| Step | Action | Expected Result | Status |
|------|--------|----------------|--------|
| 5.1 | Click "Tool Outputs" tab | Tab switches with active indicator | ⬜ |
| 5.2 | Verify tool execution cards | Each card shows: tool name, timestamp, duration | ⬜ |
| 5.3 | Check stdout display | Gray code block with tool output | ⬜ |
| 5.4 | Check stderr display | **Red text** with danger border | ⬜ |
| 5.5 | Verify exit code coloring | Green for 0, red for non-zero | ⬜ |
| 5.6 | Check FAILED badge | Red "FAILED" badge on non-zero exit | ⬜ |
| 5.7 | Expand collapsed outputs | Long outputs scrollable | ⬜ |

### Test Scenario 6: Session Detail - Attachments Tab

| Step | Action | Expected Result | Status |
|------|--------|----------------|--------|
| 6.1 | Click "Attachments" tab | Tab switches | ⬜ |
| 6.2 | Verify image previews | `<img>` tags load correctly | ⬜ |
| 6.3 | Check non-image icons | PDF/video/audio show emoji icons (📄🎬🔊) | ⬜ |
| 6.4 | Verify file metadata | Filename, size (KB/MB/GB), MIME type shown | ⬜ |
| 6.5 | Click download button | File downloads via `/api/memory/sessions/attachment/{id}` | ⬜ |
| 6.6 | Check responsive grid | 2 columns on tablet, 3-4 on desktop | ⬜ |
| 6.7 | Hover image preview | Slight zoom effect (scale 1.05) | ⬜ |

### Test Scenario 7: Search

| Step | Action | Expected Result | Status |
|------|--------|----------------|--------|
| 7.1 | Enter search query | Search bar accepts input | ⬜ |
| 7.2 | Press Enter or click search | RPC call to `memory.sessions.search` | ⬜ |
| 7.3 | Verify search results | Matching sessions with snippets displayed | ⬜ |
| 7.4 | Check result metadata | Score, snippet, full session metadata included | ⬜ |
| 7.5 | Click search result | Detail view loads for that session | ⬜ |
| 7.6 | Clear search | Returns to full list | ⬜ |

### Test Scenario 8: Performance

| Step | Action | Expected Result | Status |
|------|--------|----------------|--------|
| 8.1 | Measure initial load | < 500ms for session list RPC | ⬜ |
| 8.2 | Measure detail load | < 300ms for session detail RPC | ⬜ |
| 8.3 | Check browser console | No JavaScript errors | ⬜ |
| 8.4 | Monitor network tab | No failed requests | ⬜ |
| 8.5 | Test with 100+ sessions | Pagination prevents memory issues | ⬜ |

### Test Scenario 9: Error Handling

| Step | Action | Expected Result | Status |
|------|--------|----------------|--------|
| 9.1 | Request non-existent session | Error message displayed | ⬜ |
| 9.2 | Server offline during request | "Connection failed" error | ⬜ |
| 9.3 | Invalid filter parameters | Validation error or fallback | ⬜ |
| 9.4 | Attachment 404 | "File not found" message | ⬜ |

---

## Automated Test Script (For After Wiring)

**File**: `scripts/test-session-ui-integration.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== Aleph Session UI Integration Test ==="
echo ""

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Check if server is running
check_server() {
    echo -n "Checking if server is running... "
    if curl -s http://localhost:18790/health > /dev/null 2>&1; then
        echo -e "${GREEN}✓${NC}"
        return 0
    else
        echo -e "${RED}✗${NC}"
        echo "ERROR: Server not running. Start with: aleph-server start"
        exit 1
    fi
}

# Check if handlers are registered
check_handlers() {
    echo -n "Checking if session management handlers are registered... "

    # Test memory.sessions.list RPC
    RESPONSE=$(curl -s -X POST http://localhost:18790/ws \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc": "2.0",
            "id": 1,
            "method": "memory.sessions.list",
            "params": {"limit": 1}
        }')

    if echo "$RESPONSE" | jq -e '.result' > /dev/null 2>&1; then
        echo -e "${GREEN}✓${NC}"
        return 0
    elif echo "$RESPONSE" | jq -e '.error.message' | grep -q "Method not found"; then
        echo -e "${RED}✗${NC}"
        echo "ERROR: RPC handlers not registered!"
        echo "See docs/testing/phase-d-ui-integration-test.md for fix"
        exit 1
    else
        echo -e "${RED}✗${NC}"
        echo "Unexpected response: $RESPONSE"
        exit 1
    fi
}

# Create test session data
create_test_data() {
    echo ""
    echo "=== Creating Test Session Data ==="

    # This would create test sessions via the MarkdownSessionStore API
    # For now, we assume sessions exist from normal usage

    SESSION_COUNT=$(curl -s -X POST http://localhost:18790/ws \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc": "2.0",
            "id": 2,
            "method": "memory.sessions.list",
            "params": {"limit": 100}
        }' | jq -r '.result.total // 0')

    echo "Found $SESSION_COUNT existing sessions"

    if [ "$SESSION_COUNT" -lt 3 ]; then
        echo -e "${YELLOW}WARNING: Only $SESSION_COUNT sessions found. Some tests may fail.${NC}"
        echo "Run a few agent sessions first to generate test data."
    fi
}

# Test 1: Session List
test_session_list() {
    echo ""
    echo "=== Test 1: Session List ==="

    RESPONSE=$(curl -s -X POST http://localhost:18790/ws \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc": "2.0",
            "id": 3,
            "method": "memory.sessions.list",
            "params": {"limit": 20, "offset": 0}
        }')

    if echo "$RESPONSE" | jq -e '.result.sessions' > /dev/null 2>&1; then
        TOTAL=$(echo "$RESPONSE" | jq -r '.result.total')
        HAS_MORE=$(echo "$RESPONSE" | jq -r '.result.has_more')
        echo -e "${GREEN}✓${NC} List returned $TOTAL sessions, has_more=$HAS_MORE"
    else
        echo -e "${RED}✗${NC} List failed: $RESPONSE"
        return 1
    fi
}

# Test 2: Session Detail
test_session_detail() {
    echo ""
    echo "=== Test 2: Session Detail ==="

    # Get first session ID
    SESSION_ID=$(curl -s -X POST http://localhost:18790/ws \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc": "2.0",
            "id": 4,
            "method": "memory.sessions.list",
            "params": {"limit": 1}
        }' | jq -r '.result.sessions[0].session_id // empty')

    if [ -z "$SESSION_ID" ]; then
        echo -e "${YELLOW}⊘${NC} No sessions to test"
        return 0
    fi

    RESPONSE=$(curl -s -X POST http://localhost:18790/ws \
        -H "Content-Type: application/json" \
        -d "{
            \"jsonrpc\": \"2.0\",
            \"id\": 5,
            \"method\": \"memory.sessions.get\",
            \"params\": {\"session_id\": \"$SESSION_ID\"}
        }")

    if echo "$RESPONSE" | jq -e '.result.metadata' > /dev/null 2>&1; then
        TRANSCRIPT_LEN=$(echo "$RESPONSE" | jq -r '.result.transcript | length')
        TOOL_OUTPUT_LEN=$(echo "$RESPONSE" | jq -r '.result.tool_outputs | length')
        ATTACH_COUNT=$(echo "$RESPONSE" | jq -r '.result.attachments | length')
        echo -e "${GREEN}✓${NC} Detail loaded: transcript=$TRANSCRIPT_LEN bytes, tool_outputs=$TOOL_OUTPUT_LEN bytes, attachments=$ATTACH_COUNT"
    else
        echo -e "${RED}✗${NC} Detail failed: $RESPONSE"
        return 1
    fi
}

# Test 3: Search
test_search() {
    echo ""
    echo "=== Test 3: Full-Text Search ==="

    RESPONSE=$(curl -s -X POST http://localhost:18790/ws \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc": "2.0",
            "id": 6,
            "method": "memory.sessions.search",
            "params": {"query": "test", "limit": 10}
        }')

    if echo "$RESPONSE" | jq -e '.result.results' > /dev/null 2>&1; then
        RESULT_COUNT=$(echo "$RESPONSE" | jq -r '.result.results | length')
        echo -e "${GREEN}✓${NC} Search returned $RESULT_COUNT results"
    else
        echo -e "${RED}✗${NC} Search failed: $RESPONSE"
        return 1
    fi
}

# Test 4: Filters
test_filters() {
    echo ""
    echo "=== Test 4: Filters ==="

    # Test agent filter
    RESPONSE=$(curl -s -X POST http://localhost:18790/ws \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc": "2.0",
            "id": 7,
            "method": "memory.sessions.list",
            "params": {"agent_id": "test-agent", "limit": 10}
        }')

    if echo "$RESPONSE" | jq -e '.result.sessions' > /dev/null 2>&1; then
        echo -e "${GREEN}✓${NC} Agent filter works"
    else
        echo -e "${RED}✗${NC} Agent filter failed"
        return 1
    fi
}

# Test 5: Attachment Download
test_attachment_download() {
    echo ""
    echo "=== Test 5: Attachment Download ==="

    # Get session with attachments
    SESSION_WITH_ATTACH=$(curl -s -X POST http://localhost:18790/ws \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc": "2.0",
            "id": 8,
            "method": "memory.sessions.list",
            "params": {"limit": 100}
        }' | jq -r '.result.sessions[] | select(.attachment_count > 0) | .session_id' | head -1)

    if [ -z "$SESSION_WITH_ATTACH" ]; then
        echo -e "${YELLOW}⊘${NC} No sessions with attachments to test"
        return 0
    fi

    # Get first attachment ID
    ATTACH_ID=$(curl -s -X POST http://localhost:18790/ws \
        -H "Content-Type: application/json" \
        -d "{
            \"jsonrpc\": \"2.0\",
            \"id\": 9,
            \"method\": \"memory.sessions.get\",
            \"params\": {\"session_id\": \"$SESSION_WITH_ATTACH\"}
        }" | jq -r '.result.attachments[0].attachment_id // empty')

    if [ -z "$ATTACH_ID" ]; then
        echo -e "${YELLOW}⊘${NC} No attachment found"
        return 0
    fi

    # Test download
    HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
        "http://localhost:18790/api/memory/sessions/attachment/${SESSION_WITH_ATTACH}:${ATTACH_ID}")

    if [ "$HTTP_CODE" = "200" ]; then
        echo -e "${GREEN}✓${NC} Attachment download works (HTTP $HTTP_CODE)"
    else
        echo -e "${RED}✗${NC} Attachment download failed (HTTP $HTTP_CODE)"
        return 1
    fi
}

# Test 6: Performance
test_performance() {
    echo ""
    echo "=== Test 6: Performance ==="

    # Measure list RPC latency
    START=$(date +%s%3N)
    curl -s -X POST http://localhost:18790/ws \
        -H "Content-Type: application/json" \
        -d '{
            "jsonrpc": "2.0",
            "id": 10,
            "method": "memory.sessions.list",
            "params": {"limit": 20}
        }' > /dev/null
    END=$(date +%s%3N)
    LATENCY=$((END - START))

    if [ "$LATENCY" -lt 500 ]; then
        echo -e "${GREEN}✓${NC} List RPC latency: ${LATENCY}ms (< 500ms target)"
    else
        echo -e "${YELLOW}⚠${NC} List RPC latency: ${LATENCY}ms (> 500ms target)"
    fi
}

# Main
main() {
    check_server
    check_handlers
    create_test_data

    PASSED=0
    FAILED=0

    if test_session_list; then ((PASSED++)); else ((FAILED++)); fi
    if test_session_detail; then ((PASSED++)); else ((FAILED++)); fi
    if test_search; then ((PASSED++)); else ((FAILED++)); fi
    if test_filters; then ((PASSED++)); else ((FAILED++)); fi
    if test_attachment_download; then ((PASSED++)); else ((FAILED++)); fi
    if test_performance; then ((PASSED++)); else ((FAILED++)); fi

    echo ""
    echo "=== Test Summary ==="
    echo -e "Passed: ${GREEN}$PASSED${NC}"
    echo -e "Failed: ${RED}$FAILED${NC}"

    if [ "$FAILED" -eq 0 ]; then
        echo -e "\n${GREEN}✓ All tests passed!${NC}"
        exit 0
    else
        echo -e "\n${RED}✗ Some tests failed${NC}"
        exit 1
    fi
}

main "$@"
```

---

## Expected vs Actual Behavior

### Expected (After Fix)

1. **RPC Endpoints Available**:
   - `memory.sessions.list` → 200 OK with paginated sessions
   - `memory.sessions.get` → 200 OK with full session data
   - `memory.sessions.search` → 200 OK with FTS5 results
   - `memory.sessions.update_compression` → 200 OK
   - `GET /api/memory/sessions/attachment/:id` → Binary download

2. **Panel UI Functional**:
   - Settings → Sessions tab renders
   - Session list populates from RPC
   - Filters and pagination work
   - Detail view shows transcript/tools/attachments
   - Search returns results

### Actual (Current)

1. **RPC Endpoints Return 404**:
   ```json
   {
     "jsonrpc": "2.0",
     "id": 1,
     "error": {
       "code": -32601,
       "message": "Method not found: memory.sessions.list"
     }
   }
   ```

2. **Panel UI Non-Functional**:
   - Settings → Sessions tab may render but shows empty state
   - RPC calls fail with "Method not found"
   - Console errors: `Failed to fetch sessions`

---

## Next Steps

1. **IMMEDIATE**: Apply the fix in `src/bin/aleph-server/commands/start/mod.rs` (see Blocker section)
2. **VERIFY**: Run `cargo build` and start server
3. **TEST**: Use `scripts/test-session-ui-integration.sh` (after making it executable)
4. **MANUAL**: Walk through the manual checklist above
5. **REPORT**: Update this document with actual test results

---

## Success Criteria (From Task #25)

- [x] All UI components implemented (Tasks #21-24)
- [x] Backend RPC handlers implemented (Task #20)
- [ ] **Handlers registered during Gateway boot** ⚠️ **BLOCKER**
- [ ] Attachment HTTP route mounted
- [ ] All RPC calls succeed
- [ ] Data displays correctly in UI
- [ ] No console errors
- [ ] Performance < 500ms page load
- [ ] Manual test checklist passes
- [ ] Automated test script passes

---

**Document Status**: COMPLETE but system NOT FUNCTIONAL until blocker is resolved.
