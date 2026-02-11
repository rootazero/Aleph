# Phase 3 Alert System - Manual Testing Checklist

**Use this checklist when performing manual browser testing.**

---

## Pre-Testing Setup

- [ ] Server is running: `cargo run --bin aleph-server --features control-plane`
- [ ] Browser DevTools open (Console + Network tabs)
- [ ] URL accessible: http://127.0.0.1:18790

---

## Test 1: Initial Load

**Goal**: Verify control_plane loads and connects to Gateway

### Steps
1. [ ] Open http://127.0.0.1:18790 in browser
2. [ ] Check DevTools Console for messages:
   - [ ] "WebSocket connection established" or similar
   - [ ] No JavaScript errors
3. [ ] Check DevTools Network tab:
   - [ ] WebSocket connection to ws://127.0.0.1:18789
   - [ ] Connection status: "open" (green)
4. [ ] Verify UI loads:
   - [ ] Sidebar visible (wide mode)
   - [ ] Logo "Aleph Hub" displayed
   - [ ] Navigation items visible:
     - [ ] Dashboard
     - [ ] Agent Trace
     - [ ] System Health
     - [ ] Memory Vault
   - [ ] Settings button at bottom

### Expected Results
- ✅ UI loads without errors
- ✅ WebSocket connection established
- ✅ All navigation items visible
- ✅ No console errors

---

## Test 2: Alert Badge Display (Wide Mode)

**Goal**: Verify alert badges display correctly in wide sidebar

### Steps
1. [ ] Stay on Dashboard page (wide mode)
2. [ ] Check each navigation item for alert badges:
   - [ ] Agent Trace (alert_key: "agent.trace")
   - [ ] System Health (alert_key: "system.health")
   - [ ] Memory Vault (alert_key: "memory.status")
3. [ ] If badges visible, verify:
   - [ ] Badge positioned on top-right of icon
   - [ ] Badge color matches alert level:
     - Blue = Info
     - Yellow = Warning
     - Red = Error/Critical
   - [ ] Badge count displays (if applicable)
   - [ ] Badge is circular and small

### Expected Results
- ✅ Badges display correctly (if alerts exist)
- ✅ Badge positioning is correct
- ✅ Badge colors match alert levels
- ✅ No layout issues

---

## Test 3: Mode Switching

**Goal**: Verify smooth transition between wide and narrow modes

### Steps
1. [ ] Start on Dashboard (wide mode)
2. [ ] Note sidebar width (~256px)
3. [ ] Click "Settings" button
4. [ ] Observe transition:
   - [ ] Sidebar width changes to ~64px
   - [ ] Transition is smooth (300ms)
   - [ ] No UI flicker
   - [ ] Logo changes to icon only
   - [ ] Navigation labels disappear
5. [ ] Navigate back to Dashboard
6. [ ] Observe reverse transition:
   - [ ] Sidebar width changes to ~256px
   - [ ] Transition is smooth
   - [ ] Labels reappear

### Expected Results
- ✅ Smooth transitions (no jumps)
- ✅ Correct width changes
- ✅ No UI flicker
- ✅ Labels show/hide correctly

---

## Test 4: Tooltip Display (Narrow Mode)

**Goal**: Verify tooltip displays in narrow mode

### Steps
1. [ ] Navigate to Settings (narrow mode)
2. [ ] Hover over each navigation item:
   - [ ] Dashboard
   - [ ] Agent Trace
   - [ ] System Health
   - [ ] Memory Vault
3. [ ] For each item, verify:
   - [ ] Tooltip appears on hover
   - [ ] Tooltip shows label text
   - [ ] Tooltip positioned to the right
   - [ ] Tooltip has dark background
   - [ ] Tooltip text is readable
   - [ ] If alert exists, tooltip shows alert details
4. [ ] Move mouse away
5. [ ] Verify tooltip disappears

### Expected Results
- ✅ Tooltip appears on hover
- ✅ Tooltip positioned correctly
- ✅ Tooltip styling correct
- ✅ Tooltip disappears on mouse leave
- ✅ Alert details shown (if applicable)

---

## Test 5: WebSocket Connection

**Goal**: Verify WebSocket connection lifecycle

### Steps
1. [ ] Open DevTools Network tab
2. [ ] Filter for "WS" (WebSocket)
3. [ ] Find connection to ws://127.0.0.1:18789
4. [ ] Click on WebSocket connection
5. [ ] Check "Messages" tab
6. [ ] Verify messages:
   - [ ] Outgoing: RPC requests (JSON-RPC 2.0 format)
   - [ ] Incoming: RPC responses
   - [ ] Incoming: Event notifications (if any)
7. [ ] Check message format:
   ```json
   // Request
   {"jsonrpc":"2.0","method":"health","params":{},"id":"1"}

   // Response
   {"jsonrpc":"2.0","result":{"status":"healthy","timestamp":"..."},"id":"1"}
   ```

### Expected Results
- ✅ WebSocket connection open
- ✅ Messages sent/received
- ✅ JSON-RPC 2.0 format correct
- ✅ No connection errors

---

## Test 6: Real-Time Alert Updates (Optional)

**Goal**: Verify alerts update in real-time

### Prerequisites
- Ability to trigger system events (e.g., via Gateway RPC or system load)

### Steps
1. [ ] Monitor DevTools Console
2. [ ] Monitor WebSocket messages
3. [ ] Trigger system event (e.g., high memory usage)
4. [ ] Check for incoming event:
   ```json
   {
     "jsonrpc": "2.0",
     "method": "event",
     "params": {
       "topic": "alerts.system.health",
       "data": {
         "level": "warning",
         "count": 1,
         "message": "High CPU usage"
       }
     }
   }
   ```
5. [ ] Verify UI updates:
   - [ ] Badge appears on corresponding item
   - [ ] Badge color matches alert level
   - [ ] No delay or flicker
6. [ ] Clear alert (if possible)
7. [ ] Verify badge disappears

### Expected Results
- ✅ Event received via WebSocket
- ✅ UI updates immediately
- ✅ Badge displays correctly
- ✅ No performance issues

---

## Test 7: Error Handling

**Goal**: Verify graceful error handling

### Steps
1. [ ] Stop aleph-server
2. [ ] Observe UI behavior:
   - [ ] Connection error message (if implemented)
   - [ ] Reconnection attempts
3. [ ] Restart aleph-server
4. [ ] Verify:
   - [ ] UI reconnects automatically
   - [ ] Alert states reload
   - [ ] No data loss

### Expected Results
- ✅ Graceful disconnect handling
- ✅ Automatic reconnection
- ✅ State recovery

---

## Test 8: Performance

**Goal**: Verify UI performance

### Steps
1. [ ] Open DevTools Performance tab
2. [ ] Start recording
3. [ ] Perform actions:
   - [ ] Navigate between pages
   - [ ] Switch modes
   - [ ] Hover over items
4. [ ] Stop recording
5. [ ] Check metrics:
   - [ ] Frame rate (should be 60 FPS)
   - [ ] No long tasks (> 50ms)
   - [ ] No layout thrashing

### Expected Results
- ✅ Smooth 60 FPS
- ✅ No performance bottlenecks
- ✅ Fast response times

---

## Issues Found

**Document any issues here:**

| Issue | Severity | Description | Steps to Reproduce |
|-------|----------|-------------|-------------------|
| | | | |
| | | | |
| | | | |

---

## Screenshots

**Capture screenshots for documentation:**

1. [ ] Dashboard (wide mode) - no alerts
2. [ ] Dashboard (wide mode) - with alerts
3. [ ] Settings (narrow mode) - no alerts
4. [ ] Settings (narrow mode) - with tooltip
5. [ ] DevTools - WebSocket messages
6. [ ] DevTools - Console logs

---

## Sign-Off

- [ ] All tests completed
- [ ] Issues documented
- [ ] Screenshots captured
- [ ] Ready for Phase 3 verification

**Tested By**: _______________
**Date**: _______________
**Browser**: _______________
**OS**: _______________

---

**Checklist Version**: 1.0
**Last Updated**: 2026-02-11
