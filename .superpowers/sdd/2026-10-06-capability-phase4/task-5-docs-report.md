# Task 5 — Phase 4 documentation update

## Updated files

- `docs/reference/FEATURE_LOCATOR.md`: added §3.5d with CapabilityKind/Descriptor, Zahir facade, Tool-only live backend, ownership scopes, cursor resync, effect-claim closure, durable memo producers, fact-source layering, compatibility boundary, and deferred gates.
- `docs/reference/CODE_ORGANIZATION.md`: recorded the `src/capability/` and session durable-boundary module layout.
- `docs/reference/DESIGN_PATTERNS.md`: recorded projection-only surfaces, fail-closed owner-generation leases, and effect-sandwich/replay separation.
- `AGENTS.md` and `CLAUDE.md`: synchronized the Tier 1 Phase 4 entry and linked the new Feature Locator section; both now carry the 2026-10-06 update date.

## Boundary statements recorded

- `ToolBackendAdapter` wrapping `ToolHandlerRegistry` is the only complete live capability backend.
- Other capability kinds remain contract/deferred; ACP/MCP are projection/transport.
- Session event storage and atomic `SessionService::emit_batch` are the recovery-related committed boundary.
- StateDatabase, ACP JSON, and MCP/tool views are projection-only; GlobalBus is notification-only.
- `to_metadata_form` remains a legacy Tool compatibility wrapper.
- Automatic Safe Replay, external-effect exactly-once, universal durable scheduler, full ACP server gate, and concrete cross-backend facade wiring remain deferred.
- `src/harness/` and `src/mcp/tool_bridge.rs` remain outside this change.

## Verification

- `git diff --check`: passed.
- `rg` anchors verified for CapabilityKind, Zahir, OwnershipTree, EffectClaimReconciliation, projection-only, and deferred boundaries.
