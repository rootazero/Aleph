# SessionCapability Registration Blockers (Task B)

**Critical issues found by task-b — pre-production prerequisites**

---

## 1. Session Identity Mismatch ⚠️

**Problem:**
- `SessionService::SessionId` = `routing::session_key::SessionKey` (agent/channel/peer/task hierarchy)
- `OwnerRef::Session` / `VisibilityScope.session` = `acp::manager::SessionKey` (harness/cwd/name)
- **No conversion found** between the two types in codebase
- Cannot put raw UUID in OwnerRef

**Impact:** Session-scoped capabilities can't be properly registered

**Solution:** Establish canonical SessionKey mapping (Phase 5)

---

## 2. Subscription Incomplete ⚠️

**Problem:**
- `Zahir::describe()` enumerates ProviderRegistry ✅
- `Zahir::subscribe()` only taps ToolHandlerRegistry broadcast (zahir_facade.rs:811-824)
- Per-session provider registration changes NOT captured
- No way to watch dynamic session capability changes

**Impact:** Real-time capability discovery incomplete for sessions

**Solution:** Extend subscription to include ProviderRegistry events (Phase 5)

---

## 3. ProviderRegistry Gaps ⚠️

**Problem:**
- `ProviderRegistry::register()` exists ✅
- `ProviderRegistry::remove()` missing ❌
- Cannot clean up session-scoped capabilities on session end

**Impact:** Resource leak if sessions create providers

**Solution:** Implement removal capability (Phase 5)

---

## Phase 4 Decision

Document these blockers. SessionCapability registration **incomplete for production** but **design can be specified** with constraints.

Options:
1. **Defer Task E entirely** to Phase 5 (cleanest)
2. **Design only, no implementation** (what task-b should deliver)

Either way, don't ship broken promises.
