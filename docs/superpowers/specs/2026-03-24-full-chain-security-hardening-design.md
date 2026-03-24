# Full-Chain Security Hardening Design

**Date:** 2026-03-24
**Status:** Approved
**Scope:** Gateway + Execution + Content security layers
**Reference:** OpenClaw v0.9 security audit findings

## Background

Cross-referencing OpenClaw's comprehensive security architecture with Aleph's current implementation reveals several critical gaps. While Aleph has strong foundations (HMAC tokens, AES-256-GCM vault, three-layer exec security gate, stateless policy engine), it lacks defense-in-depth across the full request lifecycle.

This design implements a layered security hardening in three phases, ordered by attack surface priority (outside-in).

## Guiding Constraints

- **R3 (Core Minimalism):** No heavy third-party deps — rate limiter uses stdlib, no `governor` crate
- **R8 (LLM Sovereignty):** Content sanitizer marks suspicious patterns but lets LLM judge trust
- **P6 (KISS/YAGNI):** No premature trait abstractions — each feature is a concrete struct/fn
- **P7 (Defensive Design):** Lock poisoning handled, UTF-8 safe, graceful degradation

## Module Layout

### New modules

```
core/src/security/
├── mod.rs                  — Module entry
├── headers.rs              — Security response headers (tower Layer)
├── rate_limiter.rs         — Per-IP sliding window rate limiter
├── ssrf.rs                 — SSRF protection engine
├── content_sanitizer.rs    — External content boundary marking + homoglyph normalization
└── audit.rs                — Persistent security audit log (SQLite)
```

### Modified modules

```
core/src/exec/
├── kernel.rs               — Extend: env variable injection detection rules
├── risk.rs                 — Extend: new risk rules for env injection
└── sanitize.rs             — New: Unicode/invisible character sanitization

core/src/exec/approval/
└── path_security.rs        — Extend: canonical path validation

core/src/gateway/server/
└── mod.rs                  — Integrate: add tower layers (headers + rate limit)

core/src/agent_loop/
└── prompt_builder.rs       — Integrate: external content boundary injection
```

---

## Phase 1: Gateway Hardening

### 1.1 Security Response Headers (`security/headers.rs`)

A tower `Layer` that injects security headers on all HTTP responses.

**Headers applied:**

| Header | Value |
|--------|-------|
| `Content-Security-Policy` | `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self'; connect-src 'self' ws: wss:; frame-ancestors 'none'; object-src 'none'; base-uri 'none'` |
| `Strict-Transport-Security` | `max-age=31536000; includeSubDomains` |
| `X-Content-Type-Options` | `nosniff` |
| `X-Frame-Options` | `DENY` |
| `X-XSS-Protection` | `0` (modern browsers should rely on CSP) |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |
| `Permissions-Policy` | `camera=(), microphone=(), geolocation=()` |
| `Cache-Control` | `no-store` (API responses only; static assets exempt) |

**Static asset exemption:** Paths matching `/assets/`, `*.js`, `*.css`, `*.wasm` skip `Cache-Control: no-store`.

**Implementation:**
- `SecurityHeadersLayer` implements `tower::Layer`
- `SecurityHeadersService<S>` implements `tower::Service<Request<Body>>`
- In `call()`: awaits inner service, then inserts headers on response

**Integration point:** `GatewayServer::build_router()` adds `.layer(SecurityHeadersLayer::new())` as the outermost layer.

### 1.2 Request Rate Limiter (`security/rate_limiter.rs`)

In-memory sliding-window per-IP rate limiter.

```rust
pub struct GatewayRateLimiter {
    windows: Mutex<HashMap<IpAddr, SlidingWindow>>,
    config: RateLimitConfig,
}

pub struct RateLimitConfig {
    pub max_requests: u32,           // default: 100
    pub window_duration: Duration,   // default: 60s
    pub lockout_duration: Duration,  // default: 5min
    pub exempt_loopback: bool,       // default: true
}

struct SlidingWindow {
    timestamps: VecDeque<Instant>,
    locked_until: Option<Instant>,
}
```

**Behavior:**
- Loopback addresses (`127.0.0.1`, `::1`) exempt by default (personal assistant model)
- Unauthenticated requests get stricter limit (`max_requests / 4`)
- Exceeding limit returns `429 Too Many Requests` with `Retry-After` header
- Lockout state auto-expires after `lockout_duration`
- Background cleanup every 5 minutes: prune entries with no activity in the last `window_duration * 2`
- Lock poisoning: `unwrap_or_else(|e| e.into_inner())`

**Relationship to existing `rate_limiter` field:** The current `GatewaySharedState.rate_limiter` is checked for overlap. If it handles a different concern (e.g., LLM call concurrency), this new limiter coexists as a separate HTTP-level concern. If it's unused or duplicates intent, it gets replaced.

**Integration:** Axum tower Layer wrapping the router, or called explicitly in auth middleware before handler dispatch.

### 1.3 SSRF Protection Engine (`security/ssrf.rs`)

Validates URLs before any outbound HTTP request from tools.

```rust
pub fn validate_url(url: &str, policy: &SsrfPolicy) -> Result<ValidatedUrl, SsrfError>

pub struct SsrfPolicy {
    pub allow_private_network: bool,   // default: false
    pub allowed_hosts: Vec<String>,    // exact or *.wildcard patterns
}

pub struct ValidatedUrl {
    pub url: Url,
    pub resolved_ips: Vec<IpAddr>,
}
```

**Hardcoded blocklist:**
- Loopback: `127.0.0.0/8`, `::1`, `localhost`
- Private networks: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`
- Link-local: `169.254.0.0/16`, `fe80::/10`
- Cloud metadata: `169.254.169.254`, `metadata.google.internal`
- IPv4-mapped IPv6: `::ffff:127.0.0.1`, `::ffff:10.0.0.0/104`, etc.

**DNS rebinding defense:**
1. Parse URL → extract hostname
2. Resolve hostname via `tokio::net::lookup_host()`
3. Check ALL resolved IPs against blocklist
4. If any IP is blocked → reject with `SsrfError::BlockedAddress`
5. Return `ValidatedUrl` with resolved IPs for connection pinning

**Allowlist logic:**
- Exact match: `api.example.com`
- Wildcard subdomain: `*.example.com` matches `sub.api.example.com`
- Case-insensitive comparison

**Integration points:**
- `web_fetch` tool: call `validate_url()` before `reqwest::get()`
- Webhook outbound: call before posting webhook payloads
- MCP HTTP transport: call before connecting to remote MCP servers
- Exposed as `pub` function for any future tool needing external HTTP

---

## Phase 2: Execution Layer Hardening

### 2.1 Environment Variable Injection Detection (`exec/kernel.rs` extension)

New detection rules added to `SecurityKernel::assess()`.

**Dangerous environment variable blocklist:**

```
# JVM toolchain injection
MAVEN_OPTS, SBT_OPTS, GRADLE_OPTS, JAVA_TOOL_OPTIONS,
_JAVA_OPTIONS, JDK_JAVA_OPTIONS

# Dynamic linker hijacking
LD_PRELOAD, LD_LIBRARY_PATH, DYLD_INSERT_LIBRARIES,
DYLD_LIBRARY_PATH, DYLD_FRAMEWORK_PATH

# .NET hijacking
DOTNET_STARTUP_HOOKS, COR_PROFILER, COR_PROFILER_PATH,
CORECLR_PROFILER, CORECLR_PROFILER_PATH

# Node.js injection
NODE_OPTIONS

# Python injection
PYTHONSTARTUP, PYTHONPATH

# Ruby injection
RUBYOPT, RUBYLIB

# Proxy hijacking
http_proxy, https_proxy, HTTP_PROXY, HTTPS_PROXY

# Shell injection
BASH_ENV, ENV, CDPATH
```

**Detection patterns in command text:**
- `export DANGEROUS_VAR=...`
- `DANGEROUS_VAR=value command`
- `env DANGEROUS_VAR=value ...`

**Risk escalation:** Matched commands escalate to `Danger` tier (requires human approval), NOT `Blocked`. This preserves the user's right to knowingly proceed (R8 — LLM sovereignty extends to user sovereignty over their own machine).

**Implementation:** New function `check_env_injection(command: &str) -> Option<RiskEscalation>` called within existing `assess()` pipeline, after current rules. Returns reason string for the approval dialog.

### 2.2 Unicode/Invisible Character Sanitization (`exec/sanitize.rs` — new file)

```rust
/// Strip invisible/confusable characters from text for safe display
pub fn sanitize_display_text(text: &str) -> String

/// Check if text contains suspicious invisible characters
pub fn has_invisible_chars(text: &str) -> bool
```

**Filtered character classes:**

| Category | Codepoints |
|----------|------------|
| Zero-width | U+200B (ZWSP), U+200C (ZWNJ), U+200D (ZWJ), U+FEFF (BOM) |
| Word joiners | U+2060 (WJ), U+2061-2064 (math invisible operators) |
| Hangul fillers | U+3164, U+115F-1160 |
| Bidi controls | U+200E-200F (LRM/RLM), U+202A-202E (embedding), U+2066-2069 (isolates) |
| Tag characters | U+E0001-E007F (deprecated tags) |
| Variation selectors | U+FE00-FE0F |

**Integration points:**

1. **Approval display** — `ExecApprovalManager` calls `sanitize_display_text()` before showing command to user. If `has_invisible_chars()` returns true, appends warning indicator to the display.

2. **Risk escalation** — `ExecSecurityGate::pre_execute()` checks `has_invisible_chars()`. If true, escalates risk by one tier:
   - Safe → Caution
   - Caution → Danger
   - Danger/Blocked → unchanged

### 2.3 Path Canonicalization (`exec/approval/path_security.rs` extension)

Replace simple prefix matching with canonical path validation.

```rust
pub fn validate_path_in_scope(
    path: &str,
    allowed_scopes: &[PathBuf],
) -> Result<PathBuf, PathSecurityError>
```

**Improvements over current `is_path_inside()`:**

1. **Canonicalization** — `std::fs::canonicalize()` resolves symlinks and `..` segments before scope comparison
2. **Non-existent paths** — For paths that don't exist yet, manually normalize by resolving the longest existing prefix via `canonicalize()`, then appending the remaining segments with `..` collapsed
3. **URL decoding** — Percent-decode (`%2e%2e` → `..`) before validation
4. **Null byte rejection** — Reject any path containing `\0`
5. **Return canonical path** — Caller uses the validated canonical path, not the original input

**Integration:** Replace existing `is_path_inside()` calls in approval path checks with `validate_path_in_scope()`.

---

## Phase 3: Content Layer Hardening

### 3.1 External Content Boundary Marking (`security/content_sanitizer.rs`)

Wraps all untrusted external content with unique boundary markers before injection into LLM context.

```rust
pub struct ContentSanitizer;

impl ContentSanitizer {
    pub fn wrap_external_content(content: &str, source: ContentSource) -> String
    pub fn detect_injection_patterns(content: &str) -> Vec<InjectionPattern>
}

pub enum ContentSource {
    WebFetch { url: String },
    McpTool { server: String, tool: String },
    Webhook { sender: String },
    Email { from: String, subject: String },
    BrowserContent,
    UserUpload { filename: String },
}
```

**Boundary format:**

```
<<<EXTERNAL_UNTRUSTED_CONTENT id="{random_hex_8}" source="{source_type}">
{sanitized_content}
<<<END_EXTERNAL_UNTRUSTED_CONTENT id="{random_hex_8}">
```

- Random 8-byte hex ID per wrapping instance prevents boundary prediction
- Content containing `<<<EXTERNAL_` or `<<<END_EXTERNAL_` is escaped to `\<<<EXTERNAL_`

**Prompt injection pattern detection (heuristic, non-blocking):**
- `ignore previous instructions`, `you are now`, `system prompt`
- Tokenizer markers: `<|im_start|>`, `<|endoftext|>`
- Model format markers: `[INST]`, `<<SYS>>`
- Large base64 encoded blocks

Detection results are metadata only (`suspicious_patterns="N"` in boundary tag). The LLM decides trust level (R8 compliance).

**Integration:** Called in tool result processing pipeline, before results enter prompt_builder context. Each tool that fetches external content calls `ContentSanitizer::wrap_external_content()` on its output.

### 3.2 Homoglyph Normalization (in `content_sanitizer.rs`)

```rust
pub fn normalize_homoglyphs(text: &str) -> String
```

**Normalization rules:**
- Fullwidth ASCII (`Ａ-Ｚ`, `ａ-ｚ`, `０-９`) → halfwidth
- Fullwidth punctuation (`＜＞＆＂＇`) → halfwidth
- Mathematical alphanumeric symbols (`U+1D400-1D7FF`) → plain ASCII
- Common Cyrillic homoglyphs (`а→a`, `е→e`, `о→o`, `с→c`, `р→p`) → Latin equivalents

Intentionally limited scope — no full Unicode confusable table (too heavy for R3). Covers high-frequency attack vectors only.

**Called within** `wrap_external_content()` before boundary wrapping. Also reused in Phase 2's `sanitize_display_text()` for approval display.

### 3.3 Persistent Security Audit Log (`security/audit.rs`)

Extends in-memory activity logging with SQLite persistence for post-incident analysis.

```rust
pub struct SecurityAuditLog {
    sender: mpsc::Sender<AuditEntry>,  // async channel for non-blocking writes
}

pub struct AuditEntry {
    pub timestamp: i64,
    pub event_type: AuditEventType,
    pub severity: AuditSeverity,        // Critical / Warn / Info
    pub source_ip: Option<String>,
    pub session_id: Option<String>,
    pub detail: String,                 // JSON structured detail
}

pub enum AuditEventType {
    AuthFailure,
    RateLimited,
    SsrfBlocked,
    ExecBlocked,
    ExecApprovalDenied,
    InvisibleCharsDetected,
    InjectionPatternDetected,
    EnvInjectionDetected,
    PathTraversalBlocked,
}

pub enum AuditSeverity {
    Critical,
    Warn,
    Info,
}
```

**Key decisions:**

- **Reuse existing SQLite** — New `security_audit_log` table in SecurityStore's schema (new migration)
- **Async writes** — `mpsc::channel` sender in hot path, background task batches inserts (flush every 1s or 100 entries)
- **30-day retention** — Startup cleanup: `DELETE FROM security_audit_log WHERE timestamp < now - 30d`
- **Secret masking** — Detail field content passes through `SecretMasker` before storage
- **Coexists with activity_logger** — activity_logger remains for real-time UI; audit log is complementary persistent store

**Schema:**

```sql
CREATE TABLE IF NOT EXISTS security_audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    severity TEXT NOT NULL,
    source_ip TEXT,
    session_id TEXT,
    detail TEXT NOT NULL
);

CREATE INDEX idx_audit_timestamp ON security_audit_log(timestamp);
CREATE INDEX idx_audit_event_type ON security_audit_log(event_type);
```

**Integration:** `SecurityAuditLog` instance held in `GatewaySharedState`. Each security component (rate limiter, SSRF guard, exec gate, content sanitizer) calls `audit_log.log(entry)` on security events. The `log()` method is a non-blocking channel send.

---

## Implementation Order

| Step | Component | Phase | Dependencies |
|------|-----------|-------|-------------|
| 1 | `security/mod.rs` + module scaffolding | — | None |
| 2 | `security/headers.rs` | P1 | Step 1 |
| 3 | `security/rate_limiter.rs` | P1 | Step 1 |
| 4 | `security/ssrf.rs` | P1 | Step 1 |
| 5 | Gateway integration (tower layers) | P1 | Steps 2-4 |
| 6 | `exec/sanitize.rs` (Unicode) | P2 | None |
| 7 | `exec/kernel.rs` env injection rules | P2 | None |
| 8 | `exec/approval/path_security.rs` | P2 | None |
| 9 | Exec gate integration | P2 | Steps 6-8 |
| 10 | `security/content_sanitizer.rs` | P3 | Step 6 (reuse) |
| 11 | `security/audit.rs` | P3 | Step 1 |
| 12 | Full integration + audit wiring | P3 | Steps 10-11 |

## Testing Strategy

- **Unit tests** for each module (blocklist matching, boundary wrapping, rate window logic)
- **Integration tests** for gateway middleware chain (security headers present, 429 on rate limit)
- **Property tests** (proptest) for Unicode sanitization (roundtrip safety, no false positives on CJK text)
- **Exec kernel tests** for env injection detection (true positives + false negative avoidance)
- **SSRF tests** for IPv4-mapped IPv6 bypass attempts, DNS resolution mocking

## Cleanup Plan

- Remove any dead code paths replaced by new implementations
- If existing `rate_limiter` in `GatewaySharedState` is superseded, remove old implementation
- Update `docs/reference/SECURITY.md` to document new security features
- No backward compatibility shims needed (internal APIs only)
