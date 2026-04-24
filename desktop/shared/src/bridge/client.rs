//! Long-lived Swift helper RPC client.
//!
//! Implemented in Task 0.5 — spawns `aleph-bridge` as a persistent subprocess,
//! drives encode → stdin / stdout → decode → InflightTable over tokio tasks,
//! and exposes `request<P, R>(method, params, timeout)` to callers.
