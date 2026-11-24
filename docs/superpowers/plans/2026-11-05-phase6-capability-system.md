# Phase 6: Capability System Implementation

**Status:** Initiated  
**Date:** 2026-11-05  
**Based on:** Phase 5 infrastructure complete (371/371 tests passing)

## Executive Summary

Phase 6 implements the complete capability system by building on Phase 5's infrastructure. The phase is structured as 4 parallel work streams:

1. **Lifecycle Management** - Attach/detach, validation, expiration
2. **Policy Enforcement** - Access control, visibility, owner checks
3. **Discovery & Resolution** - Resource lookup, routing
4. **Cross-Session Sharing** - Delegation, grants, revocation cascades

## Phase 5 Prerequisites - Delivered ✅

| Component | Task | Status | Tests |
|-----------|------|--------|-------|
| Provider Events | Task 2 | ✅ | Zahir subscription + provider events verified |
| Type Safety | Task 3 | ✅ | SessionKey alignment (routing::SessionKey canonical) |
| Session Capability | Task 4 | ✅ | SessionCapability registration in capability layer |
| Provider Removal | Task 1 | ✅ | ProviderRegistry.unregister() committed |

**Total verification:** 371/371 capability tests passing

## Phase 6 Architecture

```
Capability System (Phase 6)
├─ Layer 1: Lifecycle Management
│  ├─ Attach/detach hooks
│  ├─ Lease validation
│  ├─ Epoch-based revocation
│  └─ Scope isolation
├─ Layer 2: Policy Enforcement
│  ├─ Access control
│  ├─ Visibility validation
│  ├─ Owner verification
│  └─ Cross-session policy
├─ Layer 3: Discovery & Resolution
│  ├─ Resource lookup
│  ├─ Visibility-scoped discovery
│  ├─ Provider-agnostic resolution
│  └─ Zahir integration
└─ Layer 4: Cross-Session Sharing
   ├─ Capability grants
   ├─ Delegation chains
   ├─ Revocation cascades
   └─ Session transition handling
```

Foundation: Phase 5 infrastructure
- Independent provider event tracking
- SessionKey canonicalization
- SessionCapability registration
- Provider removal capability

## Phase 6 Task Breakdown

### Task 1: Capability Lifecycle Management

**Objective:** Implement complete lifecycle from creation to revocation

**Deliverables:**
- `CapabilityLifecycle` trait/implementation
- Attach/detach hooks in capability providers
- Lease validation and expiration logic
- Epoch-based revocation mechanism
- Scope isolation enforcement

**Verification:**
- Lifecycle state transitions tested
- Lease expiration handled correctly
- Epoch revocation cascades properly
- Isolation boundaries enforced

**Files:**
- src/capability/lifecycle.rs (new)
- src/capability/backend.rs (modify)
- src/capability/descriptor.rs (modify)

### Task 2: Policy Enforcement Engine

**Objective:** Implement capability-based access control

**Deliverables:**
- `CapabilityPolicy` trait/implementation
- Access control decision engine
- Visibility scope validation
- Owner verification logic
- Cross-session policy checks

**Verification:**
- Access control decisions correct
- Visibility scopes enforced
- Owner claims validated
- Cross-session policies applied

**Files:**
- src/capability/policy.rs (new)
- src/capability/ownership.rs (modify)
- src/capability/descriptor.rs (modify)

### Task 3: Capability Discovery & Resolution

**Objective:** Implement resource lookup and routing

**Deliverables:**
- `CapabilityResolver` trait/implementation
- Visibility-scoped discovery logic
- Provider-agnostic resolution
- Zahir integration for discovery
- Cache and invalidation

**Verification:**
- Discovery returns correct capabilities
- Visibility scopes respected
- Resolution routes to correct providers
- Cache invalidation on events

**Files:**
- src/capability/resolver.rs (new)
- src/capability/zahir_facade.rs (modify)
- src/capability/backend.rs (modify)

### Task 4: Cross-Session Capability Sharing

**Objective:** Enable safe capability delegation between sessions

**Deliverables:**
- `CapabilityGrant` data structure
- Delegation chain tracking
- Explicit grant creation/revocation
- Revocation cascade logic
- Session transition handling

**Verification:**
- Grants created and tracked correctly
- Delegation chains validated
- Revocation cascades fully
- Session transitions preserve grants

**Files:**
- src/capability/grant.rs (new)
- src/capability/backend.rs (modify)
- src/gateway/session_manager/ops/modify.rs (modify)

## Integration Points

1. **Zahir:** Event subscription for capability lifecycle
2. **SessionManager:** Lifecycle hooks for session-scoped resources
3. **ProviderRegistry:** Provider event stream integration
4. **OwnershipTree:** Owner validation and tracking
5. **Gateway:** Session capability operations

## Success Criteria

- ✅ All 4 Phase 6 tasks complete
- ✅ 100+ new tests (all passing)
- ✅ No regressions in Phase 5
- ✅ Full capability system operational
- ✅ Ready for Phase 7 (Advanced Capabilities)

## Timeline

- **Setup:** Create feature branch, dispatch 4 teams
- **Execution:** 2-4 hours parallel work per task
- **Verification:** Full test suite + integration tests
- **Integration:** Partition and merge to main
- **Closure:** Phase 5 → Phase 6 transition complete

## Dependencies

- Phase 5 infrastructure (complete)
- Zahir subscription (complete)
- SessionKey alignment (complete)
- ProviderRegistry removal (complete)

## Risk Mitigation

- Tests drive implementation (TDD)
- Incremental commits enable rollback
- Cross-layer integration tests catch gaps
- No breaking changes to Phase 5 API

---

**Ready to initiate Phase 6 tasks on main branch.**
