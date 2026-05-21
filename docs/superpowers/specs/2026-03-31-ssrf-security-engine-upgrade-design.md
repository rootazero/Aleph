# SSRF Security Engine Upgrade Design

**Date:** 2026-03-31
**Status:** Approved
**Scope:** `src/security/ssrf/`, `src/browser/network_policy.rs`, `src/builtin_tools/web_fetch.rs`, `src/tasks/cron/webhook_target.rs`

## Problem

Aleph has two separate SSRF implementations (`security/ssrf.rs` and `browser/network_policy.rs`) with divergent coverage. Compared to OpenClaw's multi-layered SSRF defense, Aleph has critical gaps:

1. **No DNS pinning** — TOCTOU race allows DNS rebinding attacks
2. **No redirect chain validation** — reqwest auto-follows redirects without SSRF checks
3. **Webhook zero protection** — `webhook_target.rs` sends to user-provided URLs without validation
4. **Incomplete IP range coverage** — missing multicast, broadcast, TEST-NET, benchmark, IPv6 embedded IPv4 variants
5. **No legacy IPv4 literal blocking** — octal/hex/decimal/short-form bypass vectors
6. **No cross-origin header stripping** — Auth/Cookie leak on redirects
7. **Duplicated logic** — two implementations with different gaps

## Solution

Unify into a single SSRF engine in `security/ssrf/`, with `safe_fetch()` as the sole outbound HTTP request entry point.

## Architecture

```
security/ssrf/
├── mod.rs           — Public API: validate_url, validate_url_async, safe_fetch
├── ip.rs            — IP classification (all private/special-use ranges)
├── hostname.rs      — Hostname blocklist/allowlist with glob matching
├── dns.rs           — DNS resolution + pinning + rebinding defense
├── fetch.rs         — safe_fetch() with redirect chain validation
└── policy.rs        — SsrfPolicy configuration
```

### SsrfPolicy (unified configuration)

```rust
pub struct SsrfPolicy {
    /// Allow requests to private/internal IP ranges.
    /// Even when true, loopback and cloud metadata remain blocked.
    pub allow_private_network: bool,
    /// Hosts that bypass blocklist. Supports exact and wildcard (*.example.com).
    pub allowed_hosts: Vec<String>,
    /// Hosts to block. Supports glob patterns (*.malware.com).
    pub blocked_hosts: Vec<String>,
    /// Maximum redirect hops (default: 5).
    pub max_redirects: u8,
    /// Strip Authorization/Cookie on cross-origin redirects (default: true).
    pub strip_auth_on_cross_origin: bool,
}
```

### IP Classification (ip.rs)

Full coverage of non-routable/special-use ranges:

**IPv4:**
| Range | Purpose |
|-------|---------|
| 0.0.0.0/8 | Current network (unspecified) |
| 10.0.0.0/8 | RFC1918 private |
| 100.64.0.0/10 | CGNAT |
| 127.0.0.0/8 | Loopback |
| 169.254.0.0/16 | Link-local |
| 169.254.169.254/32 | Cloud metadata |
| 172.16.0.0/12 | RFC1918 private |
| 192.0.2.0/24 | TEST-NET-1 |
| 192.168.0.0/16 | RFC1918 private |
| 198.18.0.0/15 | Benchmark testing |
| 198.51.100.0/24 | TEST-NET-2 |
| 203.0.113.0/24 | TEST-NET-3 |
| 224.0.0.0/4 | Multicast |
| 240.0.0.0/4 | Reserved (includes broadcast) |

**IPv6:**
| Range | Purpose |
|-------|---------|
| ::1 | Loopback |
| :: | Unspecified |
| fe80::/10 | Link-local |
| fc00::/7 | Unique local address |
| ff00::/8 | Multicast |
| ::ffff:0:0/96 | IPv4-mapped (extract + validate inner IPv4) |
| ::x.x.x.x | IPv4-compatible deprecated (extract + validate) |
| 64:ff9b::/96 | NAT64 (extract + validate inner IPv4) |
| 64:ff9b:1::/48 | NAT64 extended |
| 2002::/16 | 6to4 (extract + validate inner IPv4) |
| 2001:0::/32 | Teredo (extract + validate inner IPv4) |

**Legacy IPv4 literal rejection** — pre-parse interception of non-standard formats:
- Octal: `0177.0.0.1`
- Hexadecimal: `0x7f000001`
- Decimal: `2130706433`
- Short-form: `127.1`
- URL credential obfuscation: `http://evil.com@127.0.0.1`

### Hostname Blocking (hostname.rs)

Hardcoded blocklist with suffix matching:
- `localhost`, `localhost.localdomain`
- `metadata.google.internal`, `metadata.internal`
- Suffixes: `.localhost`, `.local`, `.internal`

Glob-based allowlist/blocklist from policy (merged from browser module).

### DNS Pinning (dns.rs)

```
1. Async DNS resolution via tokio::net::lookup_host
2. Validate ALL returned IPs against policy
3. Return first valid IP for pinning
4. Caller uses reqwest::Client::builder().resolve(host, validated_ip)
5. No TOCTOU window — reqwest connects to pre-validated IP
```

### safe_fetch (fetch.rs)

Single entry point for all outbound HTTP requests:

```
1. URL format + scheme validation (http/https only)
2. Legacy IPv4 literal rejection
3. URL credential obfuscation detection
4. Hostname blocklist/allowlist check
5. IP literal check OR async DNS resolve + validate all IPs
6. DNS pinning via reqwest resolve()
7. Send request (redirect::Policy::none())
8. If 3xx response → redirect loop:
   a. Extract Location header
   b. Repeat steps 1-6 for new URL
   c. Cross-origin detection → strip Authorization/Cookie/Proxy-Authorization
   d. Redirect counter + loop detection (URL set dedup)
   e. Exceeds max_redirects → error
9. Return final response
```

```rust
pub async fn safe_fetch(
    url: &str,
    policy: &SsrfPolicy,
    request: SafeFetchRequest,
) -> Result<SafeFetchResponse, SsrfError>
```

`SafeFetchRequest` carries method, headers, body, timeout. `SafeFetchResponse` wraps status, headers, body bytes.

## Caller Migration

| Caller | Before | After |
|--------|--------|-------|
| `web_fetch.rs` | `validate_url()` sync + `client.get()` | `safe_fetch()` |
| `webhook_target.rs` | Bare `client.post(url)` (zero protection) | `safe_fetch()` |
| `browser/network_policy.rs` | Own `is_private_host()` + `is_private_ip()` | Calls `ssrf::validate_url()` |

## Code Cleanup

**Delete (replaced by core engine):**
- `browser/network_policy.rs`: `is_private_ip()`, `is_private_host()`, `domain_matches()`
- `web_fetch.rs`: manual `starts_with("http://")` check, direct `validate_url()` + `client.get()`

**Retain and adapt:**
- `browser/network_policy.rs`: `SsrfConfig` (browser config layer), rename `SsrfPolicy` → `BrowserSsrfGuard`, `PolicyViolation` with `From<SsrfError>`
- All existing tests (preserved, extended)

## Test Strategy

### Unit tests (ip.rs, hostname.rs, policy.rs)

Pure functions, no IO:
- All IPv4 private ranges + new ranges (TEST-NET, benchmark, multicast, broadcast)
- All IPv6 ranges + embedded IPv4 variants (NAT64, 6to4, Teredo)
- Legacy IPv4 literals (octal, hex, decimal, short-form)
- URL credential obfuscation
- Hostname blocklist suffix matching
- Allowlist/blocklist glob matching
- Policy combination edge cases

### Integration tests (dns.rs, fetch.rs)

With mock DNS / mock HTTP server:
- DNS rebinding defense (validate → pin → connect fixed IP)
- Redirect chain: public → public (allow), public → private (block)
- Redirect limit enforcement (exceed max_redirects → error)
- Redirect loop detection
- Cross-origin header stripping (same-origin retains Auth, cross-origin strips)
- Webhook target SSRF blocking
- Browser guard delegates to core engine correctly

### Regression tests

All existing tests from `ssrf.rs` and `network_policy.rs` preserved unchanged.

## Panel Configuration UI

### New Section: Outbound Request Protection (出站请求防护)

Added as a new `OutboundSecuritySection` component in `interfaces/webchat/src/views/settings/security.rs`, placed between NetworkAccessSection and PIISection.

### Configurable Settings

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| SSRF Protection | toggle | ON | Master switch. When off, all outbound requests skip SSRF checks (dangerous) |
| Allow tools to access LAN | toggle | OFF | Allow web_fetch etc. to access private addresses (10.x/172.16.x/192.168.x) |
| Allow webhooks to access LAN | toggle | OFF | Allow cron webhook delivery to private addresses |
| Max redirects | number | 5 | Maximum redirect hops for outbound requests |
| Trusted host allowlist | editable list | empty | User-defined trusted hosts (e.g. `*.corp.internal`, `nas.local`) |
| Blocked host denylist | editable list | empty | User-defined additional blocked domains |

### UI Layout

```
Security Settings Page
├── Gateway Security (existing)
├── Network Access (existing)
├── Outbound Request Protection (NEW)
│   ├── Master toggle (SSRF on/off)
│   ├── Tool LAN access toggle
│   ├── Webhook LAN access toggle
│   ├── Max redirects (number input)
│   ├── Trusted hosts (editable tag list)
│   └── Blocked hosts (editable tag list)
├── PII Protection (existing)
└── Paired Devices (existing)
```

### Data Flow

```
Panel UI
  → RPC: security_config.update({ ssrf: { enabled, allow_tool_lan, ... } })
  → Server: write to config.toml [security.ssrf] section
  → SsrfPolicy: loaded from config at request time (no restart needed)
```

### Config Schema (config.toml)

```toml
[security.ssrf]
enabled = true                    # Master switch
allow_tool_private_network = false # Tools can access LAN
allow_webhook_private_network = false # Webhooks can access LAN
max_redirects = 5
allowed_hosts = []                # ["*.corp.internal", "nas.local"]
blocked_hosts = []                # ["*.malware.com"]
```

### SecurityConfig API Extension

```rust
// In api/security.rs — extend existing SecurityConfig
pub struct SecurityConfig {
    pub require_auth: bool,
    pub enable_pairing: bool,
    pub allow_guest: bool,
    pub network_access: String,
    // NEW: SSRF outbound protection settings
    pub ssrf_enabled: bool,
    pub ssrf_allow_tool_private_network: bool,
    pub ssrf_allow_webhook_private_network: bool,
    pub ssrf_max_redirects: u8,
    pub ssrf_allowed_hosts: Vec<String>,
    pub ssrf_blocked_hosts: Vec<String>,
}
```

### Policy Construction

Each caller builds its SsrfPolicy from the config:

```rust
// web_fetch: uses tool-specific toggle
let policy = SsrfPolicy {
    allow_private_network: config.ssrf_allow_tool_private_network,
    allowed_hosts: config.ssrf_allowed_hosts.clone(),
    blocked_hosts: config.ssrf_blocked_hosts.clone(),
    max_redirects: config.ssrf_max_redirects,
    ..Default::default()
};

// webhook: uses webhook-specific toggle
let policy = SsrfPolicy {
    allow_private_network: config.ssrf_allow_webhook_private_network,
    ..same as above
};

// Master switch off → skip validation entirely (SsrfPolicy::disabled())
```

### i18n Keys (new)

```
settings.security.outbound_protection
settings.security.outbound_protection_desc
settings.security.ssrf_enabled
settings.security.ssrf_enabled_desc
settings.security.ssrf_allow_tool_lan
settings.security.ssrf_allow_tool_lan_desc
settings.security.ssrf_allow_webhook_lan
settings.security.ssrf_allow_webhook_lan_desc
settings.security.ssrf_max_redirects
settings.security.ssrf_max_redirects_desc
settings.security.ssrf_allowed_hosts
settings.security.ssrf_allowed_hosts_desc
settings.security.ssrf_allowed_hosts_placeholder
settings.security.ssrf_blocked_hosts
settings.security.ssrf_blocked_hosts_desc
settings.security.ssrf_blocked_hosts_placeholder
```

## Non-Goals

- HTTP proxy support (STRICT mode only, like OpenClaw default)
- Custom DNS resolver replacement (hickory-dns) — reqwest resolve() is sufficient
- Rate limiting on outbound requests (separate concern)
