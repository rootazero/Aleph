# Gateway remote-connection security hardening — design

**Date:** 2026-07-15
**Branch:** `worktree-gateway-remote-auth-hardening`
**Scope decision (user-approved):** Full package (F1 wire audit + F2/F3 熵减 + F4 docs + optional bind warn). Defer F5 (trusted-proxy). Audit sink = SQLite `SecurityAuditLog`.

## Context — what the audit found

Aleph's gateway remote-connection auth path (`connect::resolve_connect_auth` 4-level:
loopback → device_token → bootstrap_ticket → legacy shared token → wall) is **already
well-architected and correct**. Verified strong: raw-socket-peer loopback (no XFF spoof),
SHA-256-hashed device tokens at rest, constant-time shared-token verify
(`secret_equal_bytes`), atomic single-use bootstrap tickets, DNS-rebinding-aware
`origin_policy.rs`, per-IP rate-limit (`Auth` scope 10/60s + 5-min lockout), flood guard,
per-IP connection cap, token-rotation force-close of live remote sockets.

Gap analysis vs **openclaw** (TS) and **hermes-agent** (Py): Aleph matches or beats both on
dimensions 1–6; the two genuine gaps are (7) trusted-proxy XFF resolution (removed) and
(8) **no forensic audit trail on remote-auth events**. This is not a rotten architecture to
"deeply refactor" — the work is targeted hardening + 熵减 + doc-truth.

## Findings addressed this round

- **F1 [enhance/wire]** — `SecurityAuditLog` + `AuthFailure`/`RateLimited` event types +
  SQLite schema v7 + drain all exist, but the gateway's `audit_log` field is `Option`,
  **always `None`**. A brute-force campaign against the Gateway token is invisible. Both
  references record auth failures.
- **F2 [熵减]** — dead pairing-era audit variants `PairingAttempt`, `PairingBruteForce`,
  `GuestSessionCreated` (removed 6-digit-pairing/guest model; can never fire).
- **F3 [熵减]** — dead gateway-crypto pairing code `generate_pairing_code` +
  `PAIRING_CODE_CHARSET`/`_LENGTH` + re-exports (no consumer; flow uses `aleph-bt-<uuid>`).
- **F4 [错误修复/docs]** — `GATEWAY.md#Security` ("no authentication step / always
  operator"), `GATEWAY.md §Trusted reverse proxies` (documents removed `trusted_proxies`),
  `SECURITY.md#auth-ux` (shared-token-only + "device/pairing removed") all describe a model
  the code no longer runs.
- **F5 [defer]** — trusted-proxy XFF resolution removed; behind a reverse proxy all clients
  collapse to the proxy IP, defeating per-IP protections. Real but higher-risk (touches
  rate-limit keying; must never let XFF forge `is_loopback`). Its own cycle.

## Design — F1 wiring (SQLite sink)

**Emission points (both bounded — no rate-limiter API change):**

1. `AuditEventType::AuthFailure` on a **failed remote `connect`** (non-loopback,
   `ConnectAuthOutcome::Unauthorized`). Bounded to ≤10/60s/IP by the `Auth`-scope limiter.
   `source_ip` = client IP.
2. `AuditEventType::RateLimited` on the **flood-guard connection close**
   (`record_rejection()` → true, once per connection). Captures persistent post-connect
   probing by a walled client.

**Data flow (dedicated gateway pipeline, decoupled from guardrails):**

Rather than thread one handle across `start/mod.rs` and `orchestrator_init.rs` (multi-file
DI through two signatures + four test call sites), the gateway gets its **own**
`SecurityAuditLog` + drain, created in `start/mod.rs` where `security_store` is available.
This is the correct security behavior: auth auditing must not require guardrails to be
enabled. `spawn_audit_drain` is idempotent and safe to run alongside the guard's drain
(both append to `security_audit_log`; retention purge is a no-op for the second).

**Testable unit:** `AuditEntry::auth_failure(source_ip, detail)` /
`AuditEntry::rate_limited(source_ip, detail)` constructors in `audit.rs` (unit-tested),
called at the emission sites via `ctx.audit_log.as_ref().map(|l| l.log(entry))`.

**Files:**
- `src/security/audit.rs` — add the two `AuditEntry` constructors + tests.
- `src/gateway/server/mod.rs` — `set_audit_log` setter; populate `GatewaySharedState.audit_log`.
- `src/gateway/server/handler.rs` — `ConnectionContext.audit_log`; the two emissions.
- `src/bin/aleph-server/commands/start/mod.rs` — create gateway audit log + drain + set on server.

## Design — F2/F3 熵减

- `src/security/audit.rs` — remove the 3 dead variants + Display arms (verified zero
  consumers; enum is `#[non_exhaustive]` so external matches use wildcards).
- `src/gateway/security/crypto.rs` — remove `generate_pairing_code`,
  `PAIRING_CODE_CHARSET`, `PAIRING_CODE_LENGTH`, `test_pairing_code_format`.
- `src/gateway/security/mod.rs` — drop them from the re-export list.

## Design — F4 doc reconciliation

Rewrite three sections to the wired 4-level model; state the XFF-removed limitation honestly
(behind a reverse proxy, per-IP protections key off the proxy socket). No code behavior change.

## Design — bind warn (optional)

`src/gateway/server/mod.rs` bind site: `warn!` once when `self.addr.ip()` is non-loopback,
reminding the operator the Gateway token is the only key (parity with openclaw/hermes
fail-closed guards, but warn-not-refuse to preserve the tokenless-bootstrap flow).

## Redlines honored

R4 (gateway = pure I/O: audit is fire-and-forget logging, no business logic), R10 (no
`src/harness/` change), gateway/CLAUDE.md ("改认证/授权/Origin 必须同步更新测试"). Non-destructive:
loopback stays zero-config operator; the 4-level auth decision is untouched — only observability
is added.
