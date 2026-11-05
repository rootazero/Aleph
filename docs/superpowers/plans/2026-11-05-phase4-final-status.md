# Phase 4: Final Status & Deliverables

**Date:** 2026-11-05  
**Status:** Complete (per revised scope)

---

## Deliverables

### ✅ Task A: NSH API Verification
- Verified NSH rev 2ab3b6bc27 APIs
- Found limitations (no disabled prop, global keyboard shortcuts)
- Documented workarounds

### ✅ Task C: Server RPC Connection Plan
- File: `docs/superpowers/plans/2026-11-05-server-rpc-plan.md`
- Decision: Real AlephClient + mock for tests
- Actual RPCs: session.create, chat.send, chat.history, events.subscribe
- Missing: message.send/list/receive, session.start (mapped to chat API)
- Integration boundary: StreamEvent→NSH state reconciliation

### 🚀 Task D: UI Components (In Progress)
- Scope: `desktop/nsh-shell/src/components/**` + main.rs integration
- Components: input_field (with readonly gate), message_list, streaming display
- Constraints: No NSH UI test harness (manual QA)
- Workarounds: readonly() for disabled, global keyboard handlers

### 📋 Task B: SessionCapability Design
- File: `docs/superpowers/plans/2026-11-05-sessioncapability-design.md`
- Decision: Kind=Resource, Lifetime=Session, not desktop authority
- Blockers documented: SessionKey mismatch (routing vs acp), Zahir subscribe incomplete, ProviderRegistry missing removal
- Ready for Phase 5 once infrastructure fixes land

### ⏸️ Task E: SessionCapability Registration
- **Deferred to Phase 5** (infrastructure blockers)
- Documentation: `docs/superpowers/plans/2026-11-05-session-capability-blockers.md`

---

## Key Decisions

1. **Honest constraints over false promises**
   - Documented NSH limitations (disabled prop, keyboard shortcuts)
   - Documented capability blockers (SessionKey, subscription, removal)
   - Accepted realistic test coverage (best-effort, no harness)

2. **Real RPC over mock**
   - Use AlephClient for production
   - Mock only for deterministic tests
   - Handle schema mapping at one boundary

3. **Defer vs force**
   - Task E needs infrastructure work
   - Better to ship working UI+RPC (Phase 4)
   - Design ready for Phase 5 implementation

---

## Next Phase

Phase 5 prerequisites:
- Resolve SessionKey identity mismatch (routing vs acp)
- Extend Zahir subscription to ProviderRegistry
- Add ProviderRegistry removal capability
- Implement SessionCapability registration (Task E)

---

## Phase 4 Artifacts

All planning docs committed to `docs/superpowers/plans/2026-11-05-*`:
- nsh-api-verification.md (deprecated, corrected by Task D)
- server-rpc-plan.md ✅
- sessioncapability-design.md ✅
- session-capability-blockers.md
- phase4-scope-adjustment.md
- phase4-realities.md
- (and supporting docs)

Task D implementation code pending (in progress).
