# Design: Gateway Control Plane Hardening

## Overview
This change wires the existing auth, pairing, routing, and event subsystems into the gateway startup path so the control plane behaves consistently for both WS clients and channel inbound messages.

## Key Decisions
1. **Connection‑level auth gate**: Enforce a `connect` handshake when `require_auth` is enabled. Unauthorized requests return `AUTH_REQUIRED` and the connection is closed to align with the documented handshake expectation.
2. **Per‑connection event filters**: Use `SubscriptionManager` to maintain connection filters. Event routing matches either JSON‑RPC `method` (for `stream.*`) or `topic` fields (for `TopicEvent` payloads), with default "receive all" behavior.
3. **Unified bindings**: Reuse `AgentRouter` bindings for inbound channel routing. Inbound messages derive a channel string as `{channel_id}:{conversation_id}` to allow `channel:*` bindings while preserving per‑conversation specificity.
4. **ExecutionAdapter reuse**: Inbound routing invokes the same `ExecutionEngine` via `ExecutionAdapter`, and responses are routed back via `ReplyEmitter`.

## Compatibility Notes
- If `require_auth` is disabled, behavior remains backward compatible (no handshake required).
- Event subscription is opt‑in; existing clients that do not subscribe continue to receive all events.
