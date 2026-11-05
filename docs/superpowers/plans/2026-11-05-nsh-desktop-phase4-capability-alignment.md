# Phase 4 Implementation Plan - NSH Desktop Capability Alignment

**Date:** 2026-11-05  
**Status:** Starting  
**Based on:** Phase 3 Technical Debt + Capability System Audit

---

## 架构理解

### Aleph Capability系统

**Key Components:**
1. **CapabilityKind** (9种):
   - `Tool` ✅ (已实现 + 生产中)
   - `Skill, Agent, Task, Resource, EventSource, Subscription, Plugin, Hook` (声明但未实现)

2. **CapabilityDescriptor** (统一契约):
   ```rust
   pub struct CapabilityDescriptor {
       pub id: CapabilityId,                    // aleph/tools/xxx
       pub kind: CapabilityKind,                // which kind
       pub schema: SchemaRef,                   // sha256 fingerprint
       pub revision: CapabilityRevision,        // u64 counter
       pub lifetime: LifetimeScope,             // boot/session/etc
       pub visibility: VisibilityScope,         // scope restrictions
       pub owner: OwnerRef,                     // who owns it
       pub services: Vec<Service>,              // requires/provides/conflicts
       pub metadata: ProjectionMetadata,        // surface exposure
       pub facets: CapabilityFacets,           // Callable/EventSource/etc
   }
   ```

3. **Backend Pattern** (current):
   - `CapabilityBackend` trait: `lookup()`, `enumerate()`, `snapshot()`
   - `ToolBackendAdapter`: wraps existing `ToolHandlerRegistry`
   - `ZahirFacade`: public API layer

4. **Mounting** (Phase 4 evidence):
   - `ProjectionHost::mount_with_memory` in `src/bin/aleph-server/commands/start/mod.rs`
   - Starts one canonical `ToolHandlerRegistry` + `OwnershipTree`
   - Only **Tool backend is mounted** currently

---

## Phase 4任务分解

### Task A: NSH API Verification (Priority 1)

**Objective:** Confirm NSH rev `2ab3b6bc27` API for Phase 3 UI completion

**Approach:**
```bash
cd /Volumes/TBU4/Github/nsh && git checkout 2ab3b6bc27

# Verify these exist and understand their signatures:
rg "pub fn text_input|pub fn scroll_view|pub fn container|pub fn vertical_stack" \
  crates/nsh-core/src/ -A 3

rg "pub struct KeyEvent|pub enum Key|pub struct Modifiers" \
  crates/nsh-core/src/ -A 5
```

**Deliverable:**
- Document actual NSH API surface for Tasks 7-9
- Update `src/components/input.rs` with verified API
- Or document unavailability + alternative approach

**Estimated:** 1-2 hours (discovery only)

---

### Task B: SessionCapability Design (Priority 2a)

**Objective:** Design Session Manager as a Capability

**Current State Analysis:**
```rust
// Today: standalone module
pub struct SessionManager {
    session_id: Arc<RwLock<Option<String>>>,
    messages: Arc<RwLock<Vec<Message>>>,
    is_generating: Arc<RwLock<bool>>,
    event_tx: Option<mpsc::Sender<SessionEvent>>,
}
```

**Design Questions:**
1. Should SessionCapability be `CapabilityKind::Agent` or new kind?
2. Which `CapabilityFacet`s apply?
   - `Callable` (create_session, send_message)
   - `EventSource` (message events, streaming)
   - `Subscription` (listen to updates)
   - `ProviderLifecycle` (start/stop session)?

3. Lifetime scope?
   - `LifetimeScope::Session` (session-scoped)
   - `LifetimeScope::Process` (boot-time)?

4. Visibility/owner model?
   - Who owns the session?
   - Can multiple agents share one session?

**Deliverable:**
- Design document: `SessionCapability` contract
- Updated `session.rs` with trait definition
- Registration mechanism in Capability system

**Estimated:** 2-3 hours (design + prototype)

---

### Task C: Server RPC Connection (Priority 2b)

**Objective:** Replace Mock Client with real server RPC

**Dependency:** Verify which RPC methods are implemented

**Approach:**
1. Query server for actual RPC availability:
   ```bash
   grep -r "session\|message\|event" src/gateway/handlers/ | grep -E "async fn|pub fn"
   ```

2. If not implemented: Keep Mock Client (Risk 2 mitigation)

3. If implemented: Create `AlephRpcClient` wrapper
   ```rust
   pub struct AlephRpcClient {
       // connects to server's gateway handlers
   }
   
   #[async_trait]
   impl SessionService for AlephRpcClient {
       async fn create_session(&self) -> Result<String>;
       async fn send_message(&self, content: String) -> Result<()>;
       // etc
   }
   ```

**Deliverable:**
- RPC client implementation OR documented reason for Mock Client
- Tests with real server (if available)

**Estimated:** 2-3 hours (if server ready) / 1 hour (if mock continues)

---

### Task D: UI Components Implementation (Priority 3)

**Objective:** Complete Tasks 7-9 with verified NSH API

**Prerequisite:** Task A (NSH API verification)

**Components:**
1. **Input Field** (Task 7)
   - Multi-line text input
   - Cmd+Enter / Ctrl+Enter submit
   - Disabled state
   - Placeholder text

2. **Message List** (Task 8)
   - Scroll container
   - Render all messages
   - Role/timestamp formatting
   - Auto-scroll to bottom

3. **Streaming Display** (Task 9)
   - Real-time delta rendering
   - Progress indication
   - Word-by-word display

**Deliverable:**
- Updated `src/components/input.rs` and new `message_list.rs`
- 80%+ test coverage
- Real machine QA

**Estimated:** 4-6 hours

---

### Task E: SessionCapability Registration (Priority 4)

**Objective:** Register Session Manager as Capability in system

**Steps:**
1. Understand `CapabilityRegistry` pattern from Tool backend
2. Create `SessionCapabilityBackend` implementing `CapabilityBackend` trait
3. Mount alongside `ToolBackendAdapter` in startup
4. Test dynamic discovery + subscription

**Deliverable:**
- `SessionCapabilityBackend` implementation
- Integration tests
- Working capability discovery

**Estimated:** 3-4 hours

---

## Execution Strategy

### Week 1: System Alignment

**Day 1-2:** Task A (NSH API verification)
- Parallel with rest of team if needed
- Document findings → decide Tasks 7-9 approach

**Day 3:** Task B (SessionCapability design)
- Design review
- Get feedback on Kind/Facet/Lifetime choices

### Week 2: Implementation

**Day 1-2:** Task C (Server RPC / Mock Client decision)
- Quick query on server status
- Implement accordingly

**Day 3-4:** Task D (UI components)
- Implement Tasks 7-9 with verified API
- Real machine QA

### Week 3: Integration

**Day 1-2:** Task E (Capability registration)
- Integrate into system
- Verify dynamic discovery

**Day 3:** Documentation + cleanup
- Update progress.md
- Final verification

---

## Risk Mitigations

| Risk | Mitigation | Owner |
|------|-----------|-------|
| NSH API doesn't match assumptions | Task A discovers actual API early | Frontend task |
| Server RPC not ready | Keep Mock Client (Phase 3 already validated) | Backend task |
| Capability system too complex | Start with Task B design doc for review | Architecture |
| Performance issues in UI | Profile + optimize after implementation | Testing |

---

## Success Criteria

- [ ] NSH API verified (Task A)
- [ ] SessionCapability designed (Task B)
- [ ] Server RPC plan decided (Task C)
- [ ] UI components implemented (Task D)
- [ ] Capability registered (Task E)
- [ ] All 23 tests still passing
- [ ] Clippy clean
- [ ] Real machine QA complete

---

## Notes

**Per user guidance (2026-11-05):**
- Capability system alignment deferred from Phase 3 to Phase 4 ✓
- Rule of Three: implement Session + wait for Memory/Plugin before over-generalizing ✓
- Cost optimization: use lower-cost models for Tasks 7-9 implementation

**Team coordination:**
- Task A (NSH) could be parallel with B/C
- Tasks B/C/E are architecture-critical (review early)
- Task D (UI) can be parallel once B/C decided

---

## Next Steps

1. ✅ Read this plan
2. ⏭️ Start Task A: NSH API verification (Day 1)
3. Prepare design review for Task B (Day 2-3)

Ready to proceed?
