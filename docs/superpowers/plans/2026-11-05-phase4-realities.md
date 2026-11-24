# Phase 4: Test & RPC Findings

**Task D - Testing Reality:**
- No NSH UI test harness available
- macOS-only App (won't run tests on Linux)
- Pure logic testable, UI integration manual

**Approach:** Core logic + helpers, UI acceptance downstream

---

**Task C - Server RPC Reality:**
- Actual RPCs: `session.create`, `chat.send`, `chat.history`, `events.subscribe`
- Missing: `message.send/list/receive`, `session.start`
- Decision: Real `AlephClient` + mock for deterministic tests
- Tradeoff: StreamEvent → NSH state, history reconciliation on reconnect

**Approach:** Production RPC with fallback mock layer

---

## Integration Impact

- Task D: Testable core + manual UI QA
- Task C: Real RPC + test mocks (no pure mock)
- Task E: Waits for Task B design (independent of D/C)

Both proceeding with realistic constraints.
