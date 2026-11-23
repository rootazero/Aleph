# S1-C Web v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal / 目标:** Let an explicitly paired browser use the same Core principal over HTTPS and framed `/v1/ws` without changing legacy `/ws`.

**Architecture / 架构:** Mount a separate native-TLS ingress. Issue a short-lived opaque Cookie handle bound to the exact same-origin device credential; verify live Core state before Upgrade and dispatch using the existing bound peer and ordinary RPC lane. First slice is read-only.

**Tech stack:** Axum, existing Gateway native TLS, SecurityStore, DeviceTokenManager, Protocol v1 framing, browser Fetch/WebSocket.

**Spec:** [Accepted design §5](../specs/2026-11-22-nsh-panel-modular-refactor-design.md#5-transport-与真实能力协商--transport-and-honest-negotiation), [roadmap](2026-11-22-nsh-panel-migration-roadmap.md).

## Global constraints / 全局约束

- Separate worktree only; no old `/ws` handshake/auth changes, no implicit credentialless or owner fallback. Native HTTPS only; reverse-proxy TLS termination is not inferred from headers.
- `POST /v1/session` consumes exactly one `device_token` or `bootstrap_ticket` in JSON, up to 4 KiB. Compare single Origin with canonical `https://Host`; reject no Origin, no credential, and URL queries. Browser receives only `204` and `__Host-aleph-v1` Secure/HttpOnly/SameSite=Strict/Path=/, 300 seconds, no Domain; no credentials in URL, response body, or logs.
- `/v1/ws` requires the same exact origin and `aleph.protocol.v1`. Max payload 1 MiB, first message `system.hello`, one binary WS message per prefixed UTF-8 frame. One request serviced at a time; only `health` and `sessions.list`; no event features or retries. Limit eight v1 sockets, reserve a shared Gateway row before Upgrade, and expire independent of inbound traffic. The prior legacy two-stage admission is unchanged; this is not a mixed-transport hard-cap guarantee.
- In-memory handle registry (256 entries, lazy expiry) holds credential references, not a second grant source. Exact token revocation/expiry, device rebinding, and account deactivation fail closed. Role is read live. Native TLS remains operator configured; no deployment, UI cutover, or promotion of S1-B.
- NSH examples reviewed: `examples/host-echo/nsh.json` declares named host calls plus a Web adapter; `examples/tasks-rs/src/lib.rs` shows native registration; [Showcase](https://nsh.dev/apps/) includes Noop. Revisit for S2 runtime adapters, not for HTTP auth or evidence of locked-version support.

## Review focus / 审阅重点

- Missing, mixed, legacy, expired, revoked or unbound credentials never mint a Cookie.
- Cross-origin or cross-port use, ambiguous Cookie and non-TLS operation reject before Upgrade.
- Exact negotiated subprotocol, binary frame boundary, and hello-first gate are demonstrated by the mounted router.
- Member reads remain filtered by the actual Core caller; token revocation or expiry cannot be bypassed by an already-open socket.
- Compatibility includes actual old WS connect/health; no v1 Cookie login on that route.

### Task 1: Mount explicit credential exchange and bound Web RPC

**Files:** `src/gateway/server/connection/web_v1.rs` (new), `src/gateway/server/connection/mod.rs`, `src/gateway/server/connection/dispatch.rs`, `src/gateway/server/mod.rs`, `src/gateway/security/store/tokens.rs`.

- [x] Add mounted route regression `exchange_is_explicit_and_native_tls_only` and binding test in `web_v1/tests.rs`; valid paired member returns `204`, absent Origin, wrong Origin, mixed credential, and plaintext router fail. The focused test failed on the unmounted route before implementation.
- [x] `routes` mounts only on native TLS and is tied to the listener's cancellation token. Use canonical token manager exchange and credential references, with one write transaction for explicitly bound tickets. `WebBinding::current_role(&self) -> Option<&'static str>` verifies exact active credential, unchanged device/user, and active account. Recheck at the shared lane boundary through `DispatchPeer::web` without changing legacy dispatch.
- [x] Add mounted `web_socket_requires_hello_and_keeps_member_scope` and negative checks. Use canonical hello/framing/parse bounds and supported method allowlist; registered connections join canonical cleanup. Verify correlated response ID, empty feature list, real member scope, expiry and revoke.

### Task 2: Real browser and evidence

**Files:** `src/gateway/server/connection/web_v1/tests.rs`, `docs/reference/{DESKTOP_SHELL,FEATURE_LOCATOR}.md`, `docs/superpowers/runs/`.

- [x] Run isolated HTTPS browser with fixture credentials and scope-filtered sessions, validate host-only HttpOnly Cookie, pre-Upgrade denials, binary hello/RPC and old WS compatibility. Never use production credentials or ports.
- [x] Run serial memory-gated Gateway, security, and `gateway_ui_ingress` tests. Resolve independent review findings and record verification and outstanding gates in the [S1-C receipt](../runs/2026-11-23-nsh-panel-s1c-web-v1-verification.md). Code and evidence are separate commits.
