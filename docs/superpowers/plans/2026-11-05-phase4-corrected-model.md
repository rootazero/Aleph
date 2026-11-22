# Phase 4 Corrected Understanding (Task B Findings)

**Status:** Updating model based on investigation findings  
**Date:** 2026-11-05 (mid-Phase 4)

---

## Previous Incorrect Assumptions ❌

1. ❌ LifetimeScope has `Process` variant
2. ❌ "Only Tool backend is implemented; others are deferred"
3. ❌ SessionManager should be classified as Agent or Task kind
4. ❌ Subscription facet means "can be subscribed to"
5. ❌ ProviderLifecycle facet relates to chat start/stop

---

## Corrected Understanding ✅

### 1. LifetimeScope Actual Values
```
Runtime | Session | Run | Task | External
```
(NOT Process/Session as I stated)

### 2. Backend Architecture Status
- **ProviderRegistry + ProviderBackendAdapter** already exist (live)
- Memory is **active Resource** capability
- Tool is one backend among many, not the "only" one
- Other backends (Skills, Agents, etc.) DO have implementations

**Implication for SessionCapability:**
- Don't invent new backend pattern
- Use existing ProviderRegistry infrastructure
- SessionCapability as Resource would register through that

### 3. SessionManager Classification
- **NOT** Agent/Task kind
- **IS** session-scoped context/data
- **Kind:** Resource (transient session data)
- **Facets:** Callable (query/update), EventSource (state changes)
- **Lifetime:** Session-scoped (not Runtime)

### 4. Facet Semantics Correction
- **Callable** = invokable operations (correct)
- **EventSource** = emits state changes (correct)
- **Subscription** ≠ "can be subscribed to" — it's specific subscription service pattern
- **ProviderLifecycle** ≠ chat lifecycle — it's about provider boot/shutdown
- **ExternalEffect** = side effects beyond capability system (DB writes, etc.)

### 5. What NOT to Assume
- Don't design paths that only work in future phases
- Verify Core SessionService exists before wrapping it
- Check where session Resources actually register today

---

## Task B Current Focus

Still investigating:
- [ ] Existing Core SessionService interface
- [ ] How session-scoped Resources register in ProviderRegistry
- [ ] Registration seams / mount points
- [ ] Whether NSH Desktop session is truly Resource-scoped or needs different kind

**Wait for complete verified design before Task E implementation.**

---

## Task D and C Status

- **Task D (UI):** Proceeding normally (independent of capability design)
- **Task C (RPC):** Proceeding normally (independent of capability design)

Only Task E (SessionCapability registration) is **BLOCKED** until Task B verification completes.

---

## Next Action

✋ **HOLD Task E implementation** until task-b delivers verified design.

Task D (UI) and C (RPC) continue in parallel.
