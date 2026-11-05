# Phase 5: Infrastructure & SessionCapability Registration

**Start Date:** 2026-11-05  
**Status:** Tasks 1 & 3 dispatched (parallel), Tasks 2 & 4 pending

---

## Task Schedule

### ✅ Dispatched (Parallel)

**Task 1: ProviderRegistry.unregister()**
- Agent: phase5-task1-removal
- Status: Implementing
- Blocker for: Task 2

**Task 3: SessionKey Identity Alignment**
- Agent: phase5-task3-sessionkey  
- Status: Design phase
- Blocker for: Task 4

### ⏳ Pending (Blocked)

**Task 2: Extend Zahir Subscription**
- Blocked by: Task 1 (needs unregister event)
- Will start when: Task 1 complete
- Blocker for: Task 4

**Task 4: SessionCapability Registration**
- Blocked by: Tasks 1, 2, 3
- Will start when: All prerequisites complete

---

## Phase 5 Goals

1. ✅ Add ProviderRegistry.unregister() with broadcast
2. ✅ Extend Zahir subscription to ProviderRegistry
3. ✅ Align SessionKey types (routing vs acp)
4. ✅ Implement SessionCapability registration
5. ✅ Full E2E testing with server
6. ✅ Real machine QA

---

## Blocking Dependencies

```
Task 1 (unregister)
  ↓
Task 2 (Zahir subscription) 
  ↓
Task 4 (SessionCapability)
  ↑
Task 3 (SessionKey alignment)
```

---

## Success Criteria

- [ ] Task 1: unregister() works, tests pass, broadcast events fire
- [ ] Task 2: Zahir subscription drains both registries, tests pass
- [ ] Task 3: SessionKey conversion plan designed and approved
- [ ] Task 4: SessionCapability registered, discoverable, subscribable
- [ ] All tasks: Clippy clean, tests passing, no regressions

---

## Next Steps

1. Monitor Tasks 1 & 3 progress
2. Collect deliverables
3. Integrate Task 1 findings into Task 2
4. Integrate Task 3 findings into Task 4
5. Execute Tasks 2 & 4
6. Final integration and E2E testing
