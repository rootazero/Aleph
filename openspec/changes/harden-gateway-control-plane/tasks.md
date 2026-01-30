# Tasks: Harden Gateway Control Plane (Auth, Routing, RPC)

## 1. Implementation
- [x] Add connection‑level auth gating in `GatewayServer` and wire `AuthContext` into connection handling.
- [x] Implement `connect` handling in the WS loop to mark connections authenticated and store permissions.
- [x] Add subscription filtering for outbound events with `events.subscribe` / `events.unsubscribe` / `events.list`.
- [x] Register auth + pairing + events RPC methods in `aether_gateway` startup.
- [x] Register `agent.status` and `agent.cancel` using `ExecutionEngine` (real) and `AgentRunManager` (simulated) paths.
- [x] Start `InboundMessageRouter` on gateway startup and auto‑start channels when configured.
- [x] Unify inbound routing to use `AgentRouter` bindings and default agent selection.

## 2. Tests
- [x] Add/adjust unit tests for auth gating and subscription filtering behavior.
- [x] Add/adjust routing tests to confirm bindings are respected for inbound messages.

## 3. Documentation
- [x] Update Gateway docs/comments where behavior changes (handshake, required RPCs, bindings usage).
