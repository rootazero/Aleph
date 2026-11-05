# Phase 4: Scope Adjustment Decision

**Findings:**
- Task B: Found 3 critical blockers (SessionId mismatch, subscription incomplete, removal missing)
- Task C: RPC mapping done, real AlephClient + mocks approved
- Task D: UI implementation proceeding (input field, message list, streaming)

**Blocker Impact:**
SessionCapability registration requires Phase 5 infrastructure work. Cannot ship working Task E.

---

## Phase 4 Revised Scope

### ✅ Proceed (In Scope)
- **Task A:** NSH API verification → Done
- **Task C:** RPC integration plan → Proceed
- **Task D:** UI components 7-9 → Proceeding

### 📋 Document Only (Blocked)
- **Task B:** SessionCapability design → Document with constraints
- **Task E:** SessionCapability registration → **DEFER to Phase 5**

---

## Rationale

Better to:
1. Complete working UI + RPC integration (Users can chat)
2. Document capability design with known blockers
3. Defer registration to Phase 5 (after infrastructure in place)

Than:
1. Try to force Task E on broken foundations
2. Ship incomplete/broken capability system

---

## Action

Notify task-b: Deliver design doc with blockers listed (don't try to work around them).
Keep task-d/c proceeding (no impact on them).
Main agent will integrate D+C, defer E.
