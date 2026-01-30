# Change: Harden Gateway Control Plane (Auth, Routing, RPC)

## Why
The current gateway control plane has partial, disconnected implementations: auth handlers exist but are not enforced, key RPCs are not registered, inbound routing is implemented but never started, and bindings only affect WS agent.run while channel inbound uses a separate routing config. This makes the Gateway behave inconsistently and prevents the intended multi‑channel workflow from working end‑to‑end.

## What Changes
- Enforce connection‑level authentication when `require_auth` is enabled, including a mandatory `connect` handshake and permission gating for subsequent RPCs.
- Register missing RPC methods for auth, pairing, events subscription, and agent run control (`agent.status`, `agent.cancel`).
- Start `InboundMessageRouter` on gateway startup and wire it to the execution engine and reply emitter.
- Unify routing so channel inbound messages use the same bindings and default agent selection as WS requests.
- Provide consistent event subscription filtering for WS clients.

## Impact
- **Affected specs**: add new `gateway-control-plane` capability (new spec delta).
- **Affected code**:
  - `core/src/gateway/server.rs`
  - `core/src/bin/aether_gateway.rs`
  - `core/src/gateway/inbound_router.rs`
  - `core/src/gateway/router.rs`
  - `core/src/gateway/handlers/*` (auth/events/pairing/agent)
