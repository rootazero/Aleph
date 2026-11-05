# Phase 3 Technical Debt & Phase 4 Roadmap

**Status:** Phase 3 COMPLETE (23/23 tests passing)  
**Recorded:** 2026-11-05  
**Target for Resolution:** Phase 4

---

## Technical Debt Items

### 1. Session Manager → SessionCapability (BLOCKED on Capability system)

**Current State:**
- `desktop/nsh-shell/src/session.rs` implements `SessionManager`
- Standalone module with internal state management
- Manual event subscription via `mpsc::Receiver`

**Required Changes (Phase 4):**
- Migrate to Capability trait (`pub trait SessionCapability`)
- Register with `CapabilityRegistry`
- Support dynamic discovery, replacement, subscription via Capability system
- Update event emission to use Capability event interface

**Dependency:**
- Must understand existing Capability implementations first
- Use graphify query: "How are existing Capabilities registered and discovered?"
- Examine `src/capability/*` for patterns

**Estimated Effort:** 2-3 hours

---

### 2. UI Components Simplified (NSH API verification pending)

**Current State:**
- `src/components/input.rs` exists but is non-functional (NSH API mismatch)
- `main.rs` uses only basic `text()` component for display
- No actual input field, message list, or streaming UI rendering

**What Was Deferred:**
- Task 7: Input Field Component
  - Multi-line text input
  - Cmd+Enter / Ctrl+Enter submit
  - Disabled state during generation
  
- Task 8: Message List Component
  - Scrollable container
  - Message rendering with role/timestamp
  - Auto-scroll to bottom
  
- Task 9: Streaming Token Display
  - Real-time delta accumulation
  - Visual progress feedback

**Why Simplified:**
- NSH rev `2ab3b6bc27` API not fully verified
- Risk 1 from Plan: "NSH API mismatch"
- Mock implementation sufficient for Session Manager validation

**Required for Phase 4:**
1. **Verify NSH API** (first priority):
   ```bash
   cd /Volumes/TBU4/Github/nsh && git checkout 2ab3b6bc27
   rg "pub fn text_input|pub fn scroll_view|pub struct KeyEvent" crates/nsh-core/src/
   ```

2. **Implement Tasks 7-9** with verified API
3. **Real machine QA** when server RPC available

**Estimated Effort:** 4-6 hours (once NSH API verified)

---

### 3. Server RPC Integration (BLOCKED on server availability)

**Current State:**
- Mock Client used (all responses simulated)
- No connection to actual `src/gateway/handlers/` RPC methods

**What's Missing:**
- `session.create` RPC call
- `message.send` RPC call
- `message.list` RPC call
- Event subscription over actual transport

**Why Deferred:**
- Server RPC methods not yet implemented
- Risk 2 from Plan: "Server RPC not implemented"
- Mock Client sufficient for UI layer development

**Required for Phase 4:**
1. Verify server implements required RPCs:
   ```bash
   grep -r "session.create\|message.send\|message.list" server/src/gateway/
   ```

2. Create `AlephClient` trait wrapper
3. Replace `SessionManager`'s Mock Client with real RPC calls
4. Test with actual server

**Estimated Effort:** 3-4 hours (once server RPC ready)

---

## Phase 4 Implementation Order

### Priority 1: System Alignment (Week 1)
1. **Capability System Deep Dive**
   - Query existing implementations
   - Document registration/discovery/subscription pattern
   - Identify "SessionCapability" contract
   
2. **NSH API Verification**
   - Complete audit of rev `2ab3b6bc27`
   - Verify available UI primitives
   - Document actual API (vs. assumed API)

### Priority 2: Core Functionality (Week 2)
1. **Session Manager → SessionCapability**
   - Migrate to Capability trait
   - Register with system
   - Test dynamic replacement

2. **Complete UI Components**
   - Implement Tasks 7-9 with verified NSH API
   - 80%+ test coverage
   - Real machine QA

### Priority 3: Integration (Week 3)
1. **Server RPC Connection**
   - Replace Mock Client with real `AlephClient`
   - End-to-end test with server
   - Verify all 4 RPC methods work

2. **Memory Integration** (if scope allows)
   - Persist session history
   - Chat recovery on reconnect

---

## Related Documents

- **Plan:** `docs/superpowers/plans/2026-11-05-nsh-desktop-phase3-ui-core.md`
- **Startup Prompt:** `docs/superpowers/prompts/phase3-start-prompt.md`
- **Code:** `desktop/nsh-shell/src/session.rs`
- **Tests:** 23/23 passing in `cargo test -p aleph-nsh-shell`

---

## Notes for Phase 4 Agent

When Phase 4 starts:

1. **Read this file first** to understand deferred decisions
2. **Check the 3 Blocked dependencies**:
   - Capability system implementation patterns
   - NSH API verification (rev `2ab3b6bc27`)
   - Server RPC implementation status
3. **Use graphify** to locate existing implementations
4. **Coordinate** any changes with main Aleph architecture decisions

This debt is **well-documented and non-blocking** for Phase 3 delivery.
