# Security System

> Shell execution safety, approval workflows, and per-channel tool permissions

---

## Overview

Aleph's security system provides:
- **Permission Model**: Under LAN-trust every caller collapses to a single owner identity (see below)
- **Tool Permission Enforcement**: Per-channel tool permissions via `ScopedToolService` — governs *what an agent may do*, orthogonal to *who may connect*
- **Exec Approval**: Human-in-the-loop for shell commands
- **Command Analysis**: Static analysis of command risk
- **Allowlist/Blocklist**: Fine-grained command control
- **Output Masking**: Sensitive data protection

**Location**: `src/gateway/security/` (crypto + vault: `crypto.rs`, `shared_token.rs`, `store/`, `token_readonly.rs`), `src/tools/scoped/`, `src/exec/`

---

## Permission Model (LAN-trust)

Aleph has no authentication step — the trust boundary is the network
boundary (see [Trust model: LAN-trust](#auth-ux)). Every caller therefore
resolves to a single **owner** identity, and there is no role-based access
gate on the connection surface.

### What was removed

The role-based guest / invitation permission *engine* was deleted in the
LAN-trust revert:

- **`PolicyEngine`** (`src/gateway/security/policy_engine.rs`) — the
  per-tool `Owner` / `Guest` / `Anonymous` permission checker. Gone.
- **`InvitationManager`** (`src/gateway/security/invitation_manager.rs`)
  and the `aleph guests invite / list / revoke` CLI — guest invitation
  lifecycle. Gone.

`src/gateway/security/` now holds only crypto + vault plumbing
(`crypto.rs`, `shared_token.rs`, `store/`, `token_readonly.rs`).

### What survives (as inert types)

The identity *types* still exist in the protocol crate
(`shared/protocol/src/auth.rs`): `Role` (`Owner` / `Guest` / `Anonymous`),
`GuestScope`, and `IdentityContext`. `SessionManager` still resolves a
session's `IdentityContext` from stored metadata
(`src/gateway/session_manager/ops/identity.rs`) — but with the invitation
path gone, **every session falls back to `SessionIdentityMeta::owner`**;
nothing creates `Guest` or `Anonymous` sessions any more. Those branches
are unreachable legacy code, kept only as the audit-snapshot shape until a
later cleanup folds them away.

### Where tool permissions actually live

Limiting *what an agent may do* (as opposed to *who may connect*) is the
job of the per-channel tool-permission layer, **`ScopedToolService`**
(`src/tools/scoped/`). It merges a `ToolPermissionsConfig` across three
tiers — global → agent → channel, most-restrictive wins — and does not
read `IdentityContext`. This is orthogonal to connection trust and is
unchanged by the LAN-trust revert.

Shell-command execution safety (risk analysis, approval, allowlist,
output masking) is a separate subsystem — see **Exec Kernel** below.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                       Security System                            │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                    Exec Kernel                             │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐          │  │
│  │  │  Command   │  │   Risk     │  │  Approval  │          │  │
│  │  │  Parser    │→ │ Analyzer   │→ │  Manager   │          │  │
│  │  └────────────┘  └────────────┘  └────────────┘          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                   Approval Flow                            │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐          │  │
│  │  │  Allowlist │  │   Human    │  │   Auto     │          │  │
│  │  │  Check     │  │  Approval  │  │  Approve   │          │  │
│  │  └────────────┘  └────────────┘  └────────────┘          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                   Execution & Masking                      │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐          │  │
│  │  │  Execute   │  │   Output   │  │   Audit    │          │  │
│  │  │  Command   │  │   Masker   │  │    Log     │          │  │
│  │  └────────────┘  └────────────┘  └────────────┘          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

---

## Exec Kernel

**Location**: `src/exec/kernel.rs`

Central security enforcement for shell commands:

```rust
pub struct ExecKernel {
    parser: CommandParser,
    analyzer: RiskAnalyzer,
    approval_manager: ApprovalManager,
    allowlist: Allowlist,
    masker: OutputMasker,
}

impl ExecKernel {
    pub async fn execute(&self, command: &str) -> Result<ExecResult> {
        // 1. Parse command
        let parsed = self.parser.parse(command)?;

        // 2. Analyze risk
        let risk = self.analyzer.analyze(&parsed)?;

        // 3. Check approval
        let approval = self.get_approval(&parsed, &risk).await?;

        if !approval.approved {
            return Err(Error::NotApproved(approval.reason));
        }

        // 4. Execute
        let output = self.run_command(&parsed).await?;

        // 5. Mask sensitive output
        let masked = self.masker.mask(&output);

        Ok(masked)
    }
}
```

---

## Command Parser

**Location**: `src/exec/parser.rs`

Parse shell commands into structured form:

```rust
pub struct CommandParser;

impl CommandParser {
    pub fn parse(&self, command: &str) -> Result<ParsedCommand> {
        // Handle pipes, redirects, subshells, etc.
    }
}

pub struct ParsedCommand {
    pub program: String,
    pub args: Vec<String>,
    pub pipes: Vec<ParsedCommand>,
    pub redirects: Vec<Redirect>,
    pub env: HashMap<String, String>,
    pub is_background: bool,
}

pub struct Redirect {
    pub fd: u32,           // 0=stdin, 1=stdout, 2=stderr
    pub mode: RedirectMode,
    pub target: String,
}

pub enum RedirectMode {
    Read,     // <
    Write,    // >
    Append,   // >>
}
```

---

## Risk Analyzer

**Location**: `src/exec/risk.rs`

Evaluate command risk level:

```rust
pub struct RiskAnalyzer {
    rules: Vec<RiskRule>,
}

impl RiskAnalyzer {
    pub fn analyze(&self, cmd: &ParsedCommand) -> RiskAssessment {
        let mut level = RiskLevel::Low;
        let mut reasons = vec![];

        for rule in &self.rules {
            if rule.matches(cmd) {
                level = level.max(rule.level);
                reasons.push(rule.description.clone());
            }
        }

        RiskAssessment { level, reasons }
    }
}

pub enum RiskLevel {
    Low,       // Read-only operations
    Medium,    // File modifications
    High,      // System changes
    Critical,  // Destructive operations
}
```

### Risk Rules

| Pattern | Risk Level | Description |
|---------|------------|-------------|
| `rm -rf *` | Critical | Recursive delete |
| `chmod 777` | High | Permissive permissions |
| `curl \| sh` | Critical | Remote code execution |
| `sudo *` | High | Elevated privileges |
| `> /etc/*` | Critical | System file overwrite |
| `cat *` | Low | Read operation |
| `ls *` | Low | List operation |
| `git *` | Low | Version control |

---

## Approval Manager

**Location**: `src/exec/manager.rs`

Manage approval workflows:

```rust
pub struct ApprovalManager {
    storage: ApprovalStorage,
    bridge: ApprovalBridge,
}

impl ApprovalManager {
    pub async fn get_approval(
        &self,
        cmd: &ParsedCommand,
        risk: &RiskAssessment,
    ) -> ApprovalDecision {
        // 1. Check if already approved (session)
        if self.storage.is_approved(cmd) {
            return ApprovalDecision::approved();
        }

        // 2. Check allowlist
        if self.allowlist.is_allowed(cmd) {
            return ApprovalDecision::approved();
        }

        // 3. Check blocklist
        if self.blocklist.is_blocked(cmd) {
            return ApprovalDecision::denied("Command is blocked");
        }

        // 4. Request human approval
        self.bridge.request_approval(cmd, risk).await
    }
}
```

### Approval Decision

```rust
pub struct ApprovalDecision {
    pub approved: bool,
    pub reason: Option<String>,
    pub scope: ApprovalScope,
    pub expires_at: Option<DateTime<Utc>>,
}

pub enum ApprovalScope {
    Once,           // This execution only
    Session,        // Current session
    Permanent,      // Always allow (add to allowlist)
}
```

---

## Allowlist System

**Location**: `src/exec/allowlist.rs`

```rust
pub struct Allowlist {
    rules: Vec<AllowRule>,
}

pub struct AllowRule {
    pub pattern: String,     // Glob pattern
    pub args_pattern: Option<String>,
    pub auto_approve: bool,
}

impl Allowlist {
    pub fn is_allowed(&self, cmd: &ParsedCommand) -> bool {
        self.rules.iter().any(|rule| rule.matches(cmd))
    }
}
```

### Configuration

```json5
{
  "exec": {
    "allowlist": [
      // Always allow
      { "pattern": "ls", "autoApprove": true },
      { "pattern": "cat", "autoApprove": true },
      { "pattern": "git", "args": "status|diff|log|branch", "autoApprove": true },

      // Allow but require first-time confirmation
      { "pattern": "npm", "args": "install|run|test" },
      { "pattern": "cargo", "args": "build|test|run" }
    ],
    "blocklist": [
      { "pattern": "rm", "args": "-rf /" },
      { "pattern": "curl", "args": "* | sh" },
      { "pattern": "sudo", "args": "*" }
    ]
  }
}
```

---

## Output Masking

**Location**: `src/exec/masker.rs`

Protect sensitive data in command output:

```rust
pub struct OutputMasker {
    patterns: Vec<MaskPattern>,
}

impl OutputMasker {
    pub fn mask(&self, output: &str) -> String {
        let mut result = output.to_string();

        for pattern in &self.patterns {
            result = pattern.regex.replace_all(&result, pattern.replacement).into();
        }

        result
    }
}
```

### Masked Patterns

| Pattern | Replacement |
|---------|-------------|
| API keys | `[API_KEY_REDACTED]` |
| Passwords | `[PASSWORD_REDACTED]` |
| AWS credentials | `[AWS_CRED_REDACTED]` |
| Private keys | `[PRIVATE_KEY_REDACTED]` |
| OAuth tokens | `[TOKEN_REDACTED]` |

---

## Permission System

**Location**: `src/permission/`

### Permission Rules

```rust
pub struct PermissionRule {
    pub resource: ResourcePattern,
    pub action: Action,
    pub effect: Effect,
    pub conditions: Vec<Condition>,
}

pub enum Action {
    Read,
    Write,
    Execute,
    Delete,
    All,
}

pub enum Effect {
    Allow,
    Deny,
}

pub struct Condition {
    pub key: String,
    pub operator: ConditionOperator,
    pub value: Value,
}
```

### Resource Patterns

```
file://~/.aleph/*          # Aleph config files
file:///etc/*               # System files
exec://git/*                # Git commands
exec://npm/*                # NPM commands
network://api.openai.com/*  # OpenAI API
network://*.anthropic.com/* # Anthropic API
```

### Permission Manager

```rust
pub struct PermissionManager {
    rules: Vec<PermissionRule>,
}

impl PermissionManager {
    pub fn check(
        &self,
        resource: &str,
        action: Action,
        context: &Context,
    ) -> PermissionResult {
        for rule in &self.rules {
            if rule.matches(resource, action, context) {
                return match rule.effect {
                    Effect::Allow => PermissionResult::Allowed,
                    Effect::Deny => PermissionResult::Denied(rule.reason()),
                };
            }
        }

        // Default deny
        PermissionResult::Denied("No matching rule")
    }
}
```

---

## Audit Logging

**Location**: `src/exec/storage.rs`

All exec decisions are logged:

```rust
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub command: String,
    pub risk_level: RiskLevel,
    pub decision: ApprovalDecision,
    pub executor: String,
    pub session_key: String,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
}
```

### Audit Query

```sql
SELECT * FROM audit_log
WHERE risk_level >= 'High'
AND timestamp > datetime('now', '-7 days')
ORDER BY timestamp DESC;
```

---

## IPC Security

**Location**: `src/exec/ipc.rs`

Secure communication for approval requests:

```rust
pub struct ApprovalBridge {
    socket: UnixSocket,
}

impl ApprovalBridge {
    pub async fn request_approval(
        &self,
        cmd: &ParsedCommand,
        risk: &RiskAssessment,
    ) -> ApprovalDecision {
        // Send request to UI/CLI
        let request = ApprovalRequest {
            command: cmd.to_string(),
            risk_level: risk.level,
            reasons: risk.reasons.clone(),
        };

        self.socket.send(&request).await?;
        self.socket.recv().await
    }
}
```

---

## Security Best Practices

### For Developers

1. **Never bypass the exec kernel** - All shell execution must go through `ExecKernel`
2. **Validate inputs** - Sanitize all user-provided command arguments
3. **Use allowlists** - Prefer allowlists over blocklists
4. **Log everything** - All security decisions should be audited
5. **Principle of least privilege** - Request minimal permissions

### For Users

1. **Review approval requests** - Read commands before approving
2. **Use session scope** - Avoid permanent approvals for risky commands
3. **Check audit logs** - Regularly review what commands were executed
4. **Update allowlists** - Keep allowlists minimal and current

---

## Configuration

```json5
{
  "security": {
    "exec": {
      "enabled": true,
      "defaultPolicy": "ask",  // ask, allow, deny
      "sessionApprovals": true,
      "auditLog": true
    },
    "permissions": {
      "defaultEffect": "deny",
      "rules": [
        {
          "resource": "file://~/.aleph/*",
          "action": "read",
          "effect": "allow"
        }
      ]
    },
    "masking": {
      "enabled": true,
      "patterns": ["apiKey", "password", "secret", "token"]
    }
  }
}
```

---

## SSRF Protection (Outbound Request Security)

> Server-Side Request Forgery defense: DNS pinning, redirect chain validation, IP classification, legacy literal blocking

**Location**: `src/security/ssrf/`

### Overview

Aleph's SSRF engine validates ALL outbound HTTP requests before they leave the server. It protects against:
- Accessing private networks (10.x, 172.16.x, 192.168.x) via user-provided URLs
- Cloud metadata endpoint attacks (169.254.169.254)
- DNS rebinding (TOCTOU) attacks
- Redirect chain pivoting (public URL → private redirect target)
- Legacy IPv4 literal bypass (octal, hex, decimal encoding)
- IPv6 embedded IPv4 bypass (NAT64, 6to4, Teredo)
- URL credential obfuscation (http://evil.com@127.0.0.1)

### Architecture

```
src/security/ssrf/
├── mod.rs        — Public API: validate_url, validate_url_async, safe_fetch, SsrfError
├── policy.rs     — SsrfPolicy configuration struct
├── ip.rs         — IPv4/IPv6 classification against blocked ranges
├── hostname.rs   — Hostname blocklist, allowlist, legacy IP detection
├── dns.rs        — Async DNS resolution with address pinning
└── fetch.rs      — safe_fetch() with redirect chain validation
```

### SsrfPolicy

```rust
pub struct SsrfPolicy {
    pub enabled: bool,                    // Master switch (default: true)
    pub allow_private_network: bool,      // Allow private IPs (default: false)
    pub allowed_hosts: Vec<String>,       // Allowlist: ["*.corp.internal"]
    pub blocked_hosts: Vec<String>,       // Blocklist: ["*.malware.com"]
    pub max_redirects: u8,                // Redirect limit (default: 5)
    pub strip_auth_on_cross_origin: bool, // Strip Auth/Cookie on cross-origin redirects (default: true)
}
```

### IP Classification (`ip.rs`)

**Blocked IPv4 ranges:**

| Range | Purpose |
|-------|---------|
| `0.0.0.0/8` | Current network (unspecified) |
| `10.0.0.0/8` | RFC1918 private |
| `100.64.0.0/10` | Carrier-grade NAT |
| `127.0.0.0/8` | Loopback |
| `169.254.0.0/16` | Link-local (includes cloud metadata) |
| `172.16.0.0/12` | RFC1918 private |
| `192.0.2.0/24` | TEST-NET-1 |
| `192.168.0.0/16` | RFC1918 private |
| `198.18.0.0/15` | Benchmark testing |
| `198.51.100.0/24` | TEST-NET-2 |
| `203.0.113.0/24` | TEST-NET-3 |
| `224.0.0.0/4` | Multicast |
| `240.0.0.0/4` | Reserved + broadcast |

**Blocked IPv6 ranges:**

| Range | Purpose |
|-------|---------|
| `::1` | Loopback |
| `::` | Unspecified |
| `fe80::/10` | Link-local |
| `fc00::/7` | Unique local address |
| `ff00::/8` | Multicast |

**IPv6 embedded IPv4 extraction** — extracts inner IPv4 and validates:

| Format | Example | Extraction |
|--------|---------|------------|
| IPv4-mapped | `::ffff:127.0.0.1` | Direct mapping |
| NAT64 | `64:ff9b::7f00:1` | Last 32 bits |
| 6to4 | `2002:7f00:0001::` | Segments 1-2 |
| Teredo | `2001:0000::80ff:fffe` | Last 32 bits XOR `0xFFFFFFFF` |
| IPv4-compatible | `::127.0.0.1` | Last 32 bits |

### Hostname Blocking (`hostname.rs`)

**Hardcoded blocklist:**
- `localhost`, `localhost.localdomain`
- `metadata.google.internal`, `metadata.internal`
- Suffixes: `.localhost`, `.local`, `.internal`

**Legacy IPv4 literal detection** — blocks non-standard IP formats that bypass naive parsing:
- Octal: `0177.0.0.1` → 127.0.0.1
- Hexadecimal: `0x7f000001` → 127.0.0.1
- Decimal: `2130706433` → 127.0.0.1
- Short-form: `127.1` → 127.0.0.1

**URL credential obfuscation** — detects `http://evil.com@127.0.0.1:8080/` patterns.

### DNS Pinning (`dns.rs`)

Prevents DNS rebinding (TOCTOU) attacks:

```
1. Async DNS resolution via tokio::net::lookup_host
2. Validate ALL returned IPs against policy
3. Return first valid SocketAddr
4. Caller pins via reqwest::Client::builder().resolve(host, validated_addr)
5. No TOCTOU window — reqwest connects to pre-validated IP
```

### safe_fetch (`fetch.rs`)

**Single entry point for all outbound HTTP requests.** Replaces direct `reqwest::Client` usage.

```rust
pub async fn safe_fetch(
    url: &str,
    policy: &SsrfPolicy,
    request: SafeFetchRequest,
) -> Result<SafeFetchResponse, SsrfError>
```

**Execution flow:**

```
1. URL format + scheme validation (http/https only)
2. Legacy IPv4 literal rejection
3. URL credential obfuscation detection
4. Hostname blocklist/allowlist check
5. IP literal check OR async DNS resolve + validate all IPs
6. DNS pinning via reqwest resolve()
7. Send request (redirect::Policy::none())
8. If 3xx → redirect loop:
   a. Extract Location header
   b. Repeat steps 1-6 for new URL
   c. Cross-origin → strip Authorization/Cookie/Proxy-Authorization
   d. Redirect counter + loop detection (URL set dedup)
   e. Exceeds max_redirects → SsrfError::TooManyRedirects
9. Return final response
```

### Callers

All outbound HTTP requests go through the SSRF engine:

| Caller | File | Method |
|--------|------|--------|
| Web fetch tool | `builtin_tools/web_fetch.rs` | `safe_fetch()` |
| Webhook delivery | `tasks/cron/webhook_target.rs` | `safe_fetch()` |
| Media downloader | `gateway/pipeline/media_download.rs` | `safe_fetch()` |
| MCP HTTP transport | `mcp/transport/http.rs` | `validate_url()` |
| Browser navigation | `browser/network_policy.rs` | `validate_url()` via `BrowserSsrfGuard` |

### Browser SSRF Guard

**Location**: `src/browser/network_policy.rs`

Thin wrapper over the core SSRF engine with browser-specific features:

```rust
pub struct BrowserSsrfGuard {
    config: SsrfConfig,  // block_private, blocked_domains, allowed_domains
}

impl BrowserSsrfGuard {
    pub fn check_url(&self, url: &str) -> Result<(), PolicyViolation> {
        // Delegates to ssrf::validate_url() with converted policy
        // Adds browser-specific allowlist-only mode
    }
}
```

### Content Sanitization

**Location**: `src/security/content_sanitizer.rs`

Wraps fetched external content with boundary markers to prevent prompt injection:

```rust
pub fn wrap_external_content(content: &str, source: ContentSource) -> String
```

### Audit Logging

**Location**: `src/security/audit.rs`

SSRF blocks are logged with context, hostname, and rejection reason for security monitoring.

### Panel Configuration

Users configure SSRF settings via the Panel UI (Settings → Security → Outbound Request Protection):

| Setting | Config Key | Default |
|---------|-----------|---------|
| Enable SSRF Protection | `security.ssrf.enabled` | `true` |
| Allow tools to access LAN | `security.ssrf.allow_tool_private_network` | `false` |
| Allow webhooks to access LAN | `security.ssrf.allow_webhook_private_network` | `false` |
| Max redirects | `security.ssrf.max_redirects` | `5` |
| Trusted host allowlist | `security.ssrf.allowed_hosts` | `[]` |
| Blocked host denylist | `security.ssrf.blocked_hosts` | `[]` |

**Config file** (`~/.aleph/config.toml`):

```toml
[security.ssrf]
enabled = true
allow_tool_private_network = false
allow_webhook_private_network = false
max_redirects = 5
allowed_hosts = ["*.corp.internal"]
blocked_hosts = ["*.malware.com"]
```

**RPC**: `security_config.get` / `security_config.update` — read/write via Gateway JSON-RPC.

### Security Guarantees

1. **Fail-closed** — malformed URLs, unresolvable hosts, and unknown IP formats are rejected
2. **DNS pinning** — eliminates TOCTOU race conditions between validation and connection
3. **Every hop validated** — redirect targets are validated with the same rigor as the initial URL
4. **Header isolation** — Authorization/Cookie headers stripped on cross-origin redirects
5. **Policy-aware** — `allow_private_network=true` still blocks loopback and cloud metadata
6. **Unified engine** — single implementation prevents coverage gaps between callers

### For Developers

1. **Always use `safe_fetch()`** for outbound HTTP requests — never use `reqwest::Client` directly
2. **Use `validate_url()`** for URL-only validation without fetching (e.g., browser navigation)
3. **Construct caller-specific policies** — tools and webhooks may have different `allow_private_network` settings based on user configuration
4. **Add new callers** — any new outbound HTTP code must go through `safe_fetch()` or `validate_url()`
5. **Test coverage** — add tests for new IP ranges or bypass vectors in `ip.rs` and `hostname.rs`

---

## Trust model: LAN-trust {#auth-ux}

Aleph has **no authentication step**. The trust boundary *is* the network
boundary: whoever can reach the gateway socket is the owner. Every
`connect` handshake is accepted as `operator`
(`src/gateway/handlers/connect.rs`) — legacy `token` / `device_name`
params from old clients are accepted and ignored, never validated.

### Network boundary = trust boundary

- **Default — loopback only.** `aleph-server` binds `127.0.0.1`
  (`GatewayServerConfig::default`), so only processes on the same machine
  can connect. A single-machine desktop install needs zero configuration.
- **LAN opt-in.** Set `[gateway] host = "0.0.0.0"` in
  `~/.aleph/config.toml` to listen on every interface. **This grants every
  device on your LAN complete control over the agent** — including the
  PTY / shell-execution tools. There is no method-level gate to fall back
  on: the old per-RPC operator-vs-guest authorization
  (`method_authz`) is inert under LAN-trust because the caller role is
  always `operator`. Only enable `0.0.0.0` on a network you trust
  end-to-end (home LAN behind your router), never on an untrusted or
  public segment.
- **Beyond the LAN.** To reach Aleph across the internet, front it with
  your own reverse proxy / VPN / SSH tunnel that terminates trust
  *upstream* of the gateway. Aleph itself adds no transport auth.

### The one remaining guardrail: WS Origin check

Browsers attach an unforgeable `Origin` header to every WebSocket upgrade
and cross-origin `fetch`. A malicious public web page the user happens to
visit can still *reach* `ws://127.0.0.1:18790/ws` (loopback is not
firewalled from the browser), so without an origin check it could open a
control channel to the local daemon — the classic DNS-rebinding /
cross-origin-WebSocket confused-deputy. The origin gate
(`src/gateway/origin_policy.rs`) is therefore retained as the **only**
validation on the browser surface. It guards against the public internet,
**not** against LAN neighbours.

`OriginPolicy::is_allowed` decides:

| Origin | Verdict | Why |
|--------|---------|-----|
| **absent / empty** | allow | Native clients (CLI, bots, bridges, `tokio-tungstenite`) send none; only browsers do. |
| **loopback** (`127.0.0.0/8`, `::1`, `localhost`, `*.localhost`) | allow | Same-machine UI. |
| **`tauri:` scheme** | allow | The desktop shell's own webview origin, unspoofable by a remote page. |
| **exact allow-list match** (`[gateway] allowed_origins`) | allow | Operator-configured extra origins for split panel/API deployments. |
| **same-origin** (Origin authority == request `Host`) | allow | Remote deployments served from their own domain work without config; blocks the cross-origin confused-deputy (a page at `evil.com` carries `Origin: …evil.com`, never matching the gateway's own Host). |
| anything else (public web domain) | **deny** | |

Note the gate does **not** auto-allow arbitrary private-LAN IPs by range —
a cross-origin browser request from another LAN host is only accepted when
it is same-origin with the gateway's `Host`, in the allow-list, or
loopback/`tauri:`. Native LAN clients carry no `Origin` and pass freely.

> **Limitation — classic DNS-rebinding.** The same-origin rule defeats the
> cross-origin confused-deputy, but not classic DNS-rebinding: if an attacker
> rebinds `evil.com` to the gateway's own address, the victim's page then
> carries `Origin == Host == evil.com` and passes. Fully closing this needs a
> **Host allow-list** (rejecting `Host` headers whose name isn't a known
> gateway name), which Aleph does not currently enforce. Under the default
> loopback bind this requires a targeted local attack; it is most relevant in
> LAN mode (`host = "0.0.0.0"`).

**Escape hatch — `allow_any_origin`.** Set `[gateway] allow_any_origin =
true` to trust every Origin unconditionally
(`OriginPolicy::allow_any`). Intended only for deployments that front the
gateway with their own reverse proxy / auth layer; it leaves the agent
drivable by any web page the user's browser visits, so keep it `false`
unless you know why.

### Migration from the pre-revert auth model

The device-authentication / pairing / token system (silent bootstrap,
`/pair` 6-digit codes, `/login` form, `?token=` URLs, `aleph auth …` CLI)
was removed in the LAN-trust revert. For operators upgrading:

- **Old `[gateway.auth]` config is ignored, not rejected.** The config
  root has no `deny_unknown_fields`, so dead keys (`require_auth`,
  `enable_pairing`, `[gateway.auth]`, `[gateway.bootstrap]`, …) load
  without error and are silently dropped
  (`GatewayConfig::from_toml` legacy-config test). **One gotcha:** a
  `allowed_origins` list that lived under the legacy `[gateway.auth]`
  table is **not** migrated — move it up to the `[gateway]` root yourself.
- **`aleph auth *` CLI subcommands are gone.** There is no
  `aleph auth show-token` / `debug show-token` / `devices` / `pairing`
  command any more. "Open in Browser" survives as a desktop-shell menu
  item that hands the plain gateway URL (loopback, or the configured
  remote) to the system browser — no nonce, no token.
- **Orphaned `~/.aleph` token / device data is left in place.** The server
  ignores any leftover device/token rows on startup and never deletes user
  data; you may remove them by hand if you wish.

---

## Cross-Cutting Security Module Index

| Module | Location | Purpose |
|--------|----------|---------|
| SSRF Engine | `src/security/ssrf/` | Outbound request validation |
| HTTP Headers | `src/security/headers.rs` | Security response headers |
| Content Sanitizer | `src/security/content_sanitizer.rs` | Prompt injection defense |
| Audit Logger | `src/security/audit.rs` | Security event logging |
| Browser Guard | `src/browser/network_policy.rs` | Browser navigation SSRF |
| Crypto + Vault | `src/gateway/security/` | Secrets vault, shared-token crypto |
| Tool Permissions | `src/tools/scoped/` | Per-channel tool permission merge |
| Exec Kernel | `src/exec/` | Shell command safety |

---

## Swift helper isolation

**The Swift `AlephBridge` helper process must not read or write any vault path,
including `~/.aleph/data/`, `~/.aleph/.shared_token`, or any session or memory
storage directory.** The vault's file lock is owned and held exclusively by the
Rust core. Concurrent writes from any second writer — including the bridge
helper — corrupt the encrypted vault and unrecoverably destroy all stored API
keys, OAuth tokens, and embedding keys.

This rule has already caused a production incident (the `.shared_token` event
documented in CLAUDE.md). It must be treated as a hard constraint, not a
guideline.

Audit procedure: whenever a new handler is added to `AlephBridge`, the code
reviewer must confirm the handler does not open, read, or write any file under
`~/.aleph/`. The handler source file must include a comment of the form:

```swift
// Vault access: none. This handler only reads [describe what it reads].
```

Any handler that cannot include this comment must be redesigned before merging.

## Cross-process safety guarantees (Spec C, 2026-05-02)

The original `.shared_token` corruption incident (documented elsewhere
in this file) had a structural cause: nothing prevented two
`aleph-server` processes from writing the same `~/.aleph/data/` files.
Spec C closes that gap with four layered protections:

- **Singleton lock**: `aleph-server start` acquires
  `~/.aleph/data/aleph.lock` via `flock()` in `main()` before any
  other state. The OS releases the lock automatically on process
  exit (graceful, panic, SIGKILL). A second `start` exits cleanly
  with code 64 + a stderr diagnostic naming the holder PID.
- **Vault writes**: every write to `secrets.vault` goes through
  `alephcore::utils::vault_io::VaultIo`, which combines an fcntl
  exclusive lock (defense-in-depth even if the singleton ever fails)
  with `tempfile + persist` rename for atomicity.
- **JSON state writes**: `acp_sessions.json` and equivalent
  hand-managed JSON files use `alephcore::utils::atomic_io::write_atomic`,
  which is also temp + rename via `tempfile`.
- **SQLite connections**: every connection under `~/.aleph/data/`
  is opened via `alephcore::utils::sqlite_open::open_sqlite_safe`,
  which sets `journal_mode=WAL`, `busy_timeout=5000`, and
  `synchronous=NORMAL`. Read-only callers use `open_sqlite_readonly`.
  Multi-reader + single-writer is now safe by construction.
- **CLI dispatch**: every CLI subcommand declares a
  `CommandPolicy` (NoLock / LockOnly / LockOrIpc) and routes
  through `alephcore::cli::policy::with_policy` or `run_no_lock`.
  When the server holds the singleton lock, write subcommands
  forward to `/v1/admin/*` IPC endpoints (token rotation
  self-heals via a single 401 retry). When no server is running,
  the CLI takes the lock locally and operates directly.

Reverse-regression: `scripts/spec_c_regression.sh` enforces the four
invariants (no direct rusqlite open, no direct fs ops on vault/acp
files, every CLI subcommand annotated, no leftover
`acquire_instance_lock` callers). Run before any commit that touches
`src/utils/`, `src/cli/`, `src/gateway/admin_api/`, or
`src/bin/aleph-server/commands/`.

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Tool System](TOOL_SYSTEM.md) - How bash_exec works
- [Gateway](GATEWAY.md) - Security RPC methods
- [Desktop Bridge](DESKTOP_BRIDGE.md) - Bridge isolation invariants
