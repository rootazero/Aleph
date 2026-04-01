# Security System

> Shell execution safety, approval workflows, identity-based permission enforcement, and guest access control

---

## Overview

Aleph's security system provides:
- **Identity Context**: Immutable identity snapshots for permission enforcement
- **Guest Access Control**: Invitation-based temporary access with scoped permissions
- **Tool Permission Enforcement**: Role-based access control for all tool executions
- **Exec Approval**: Human-in-the-loop for shell commands
- **Command Analysis**: Static analysis of command risk
- **Allowlist/Blocklist**: Fine-grained command control
- **Output Masking**: Sensitive data protection

**Location**: `src/gateway/security/`, `src/exec/`

---

## Identity Context & Permission Enforcement

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                  Identity-Based Permission Flow                  │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                  Session Creation                          │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐          │  │
│  │  │ Invitation │→ │  Activate  │→ │  Identity  │          │  │
│  │  │  Manager   │  │  Session   │  │  Context   │          │  │
│  │  └────────────┘  └────────────┘  └────────────┘          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                  Execution Chain                           │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐          │  │
│  │  │   Agent    │→ │  Executor  │→ │   Policy   │          │  │
│  │  │   Loop     │  │  (Single/  │  │   Engine   │          │  │
│  │  │            │  │   Routed)  │  │            │          │  │
│  │  └────────────┘  └────────────┘  └────────────┘          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │                  Permission Check                          │  │
│  │  ┌────────────┐  ┌────────────┐  ┌────────────┐          │  │
│  │  │   Owner    │  │   Guest    │  │ Anonymous  │          │  │
│  │  │  (Allow)   │  │  (Scope)   │  │  (Deny)    │          │  │
│  │  └────────────┘  └────────────┘  └────────────┘          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### IdentityContext

**Location**: `shared/protocol/src/auth.rs`

Immutable identity snapshot that flows through the execution chain:

```rust
pub struct IdentityContext {
    /// Unique request identifier
    pub request_id: String,

    /// Session key for this request
    pub session_key: String,

    /// Role of the requester
    pub role: Role,

    /// Identity ID (\"owner\" or guest_id)
    pub identity_id: String,

    /// Guest permission scope (frozen at session creation)
    pub scope: Option<GuestScope>,

    /// Request creation timestamp (Unix seconds, UTC)
    pub created_at: i64,

    /// Source channel (\"cli\", \"gateway\", \"telegram\", etc.)
    pub source_channel: String,
}

pub enum Role {
    Owner,      // Full access to all tools
    Guest,      // Limited access based on scope
    Anonymous,  // No access (authentication required)
}
```

### Guest Scope

**Location**: `shared/protocol/src/auth.rs`

Defines what a guest can access:

```rust
pub struct GuestScope {
    /// Allowed tool names or categories
    /// Examples: [\"translate\"], [\"shell\"], [\"*\"]
    pub allowed_tools: Vec<String>,

    /// Token expiration timestamp (Unix seconds, UTC)
    pub expires_at: Option<i64>,

    /// Human-readable name for UI display
    pub display_name: Option<String>,
}
```

### Permission Matching Rules

1. **Exact Match**: `\"translate\"` matches tool `\"translate\"`
2. **Category Match**: `\"shell\"` matches `\"shell:exec\"`, `\"shell:read\"`, etc.
3. **Wildcard**: `\"*\"` matches any tool

### PolicyEngine

**Location**: `src/gateway/security/policy_engine.rs`

Stateless permission checker:

```rust
impl PolicyEngine {
    /// Check if identity has permission to execute a tool
    pub fn check_tool_permission(
        identity: &IdentityContext,
        tool_name: &str,
    ) -> PermissionResult {
        match identity.role {
            Role::Owner => PermissionResult::Allowed,

            Role::Guest => {
                // Check scope, expiration, and tool permission
                Self::check_guest_scope(scope, tool_name, guest_id)
            }

            Role::Anonymous => PermissionResult::Denied {
                reason: \"Authentication required\".to_string(),
            },
        }
    }
}

pub enum PermissionResult {
    Allowed,
    Denied { reason: String },
}
```

### Invitation Manager

**Location**: `src/gateway/security/invitation_manager.rs`

Manages guest invitation lifecycle:

```rust
impl InvitationManager {
    /// Create a new guest invitation
    pub fn create_invitation(
        &self,
        request: CreateInvitationRequest,
    ) -> Result<Invitation, InvitationError> {
        // Generate unique token and guest_id
        // Store pending invitation
        // Return invitation with URL
    }

    /// Activate an invitation (one-time use)
    pub fn activate_invitation(
        &self,
        token: &str,
    ) -> Result<GuestToken, InvitationError> {
        // Validate token
        // Check expiration
        // Mark as activated
        // Return guest token with scope
    }
}
```

### Session Identity Metadata

**Location**: `src/gateway/session_manager.rs`

Identity metadata stored in session database:

```rust
pub struct SessionIdentityMeta {
    /// Role of the session owner
    pub role: Role,

    /// Identity ID (\"owner\" or guest_id)
    pub identity_id: String,

    /// Guest scope (frozen at session creation)
    pub scope: Option<GuestScope>,

    /// Source channel
    pub source_channel: String,
}
```

### Execution Flow

1. **Session Creation**
   - Owner: Default identity with full access
   - Guest: Activate invitation → Store identity in session metadata

2. **Request Processing**
   - Gateway receives request with session_key
   - SessionManager constructs IdentityContext from metadata
   - IdentityContext passed to AgentLoop

3. **Tool Execution**
   - AgentLoop passes IdentityContext to Executor
   - Executor checks permission via PolicyEngine
   - If allowed: Execute tool
   - If denied: Return ToolError with reason

### Example: Guest Invitation Flow

```rust
// 1. Create invitation (Owner only)
let scope = GuestScope {
    allowed_tools: vec![\"translate\".to_string()],
    expires_at: Some(now + 3600), // 1 hour
    display_name: Some(\"Mom\".to_string()),
};

let invitation = manager.create_invitation(CreateInvitationRequest {
    guest_name: \"Mom\".to_string(),
    scope,
})?;

// invitation.token: \"abc123...\"
// invitation.url: \"https://aleph.local/join?t=abc123...\"

// 2. Guest activates invitation
let guest_token = manager.activate_invitation(&invitation.token)?;

// 3. Guest creates session with token
// SessionManager stores identity metadata

// 4. Guest executes tool
let identity = session_manager.get_identity_context(&session_key, \"gateway\")?;
// identity.role = Role::Guest
// identity.scope = Some(GuestScope { allowed_tools: [\"translate\"], ... })

let result = executor.execute(&action, &identity).await;
// If action.tool_name = \"translate\" → Allowed
// If action.tool_name = \"shell_exec\" → Denied
```

### CLI Commands

```bash
# Create guest invitation
aleph guests invite --scope translate --ttl 30d --name \"Mom\"

# List pending invitations
aleph guests list

# Revoke invitation
aleph guests revoke <guest_id>
```

### Security Guarantees

1. **Immutability**: IdentityContext is immutable once created
2. **Frozen Permissions**: Guest scope is frozen at session creation
3. **One-Time Use**: Invitations can only be activated once
4. **Expiration**: Both invitations and guest tokens can expire
5. **Stateless Checks**: PolicyEngine has no mutable state
6. **Audit Trail**: All permission checks are logged

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

## Cross-Cutting Security Module Index

| Module | Location | Purpose |
|--------|----------|---------|
| SSRF Engine | `src/security/ssrf/` | Outbound request validation |
| HTTP Headers | `src/security/headers.rs` | Security response headers |
| Content Sanitizer | `src/security/content_sanitizer.rs` | Prompt injection defense |
| Audit Logger | `src/security/audit.rs` | Security event logging |
| Browser Guard | `src/browser/network_policy.rs` | Browser navigation SSRF |
| Identity/Auth | `src/gateway/security/` | Session, pairing, permissions |
| Exec Kernel | `src/exec/` | Shell command safety |
| Policy Engine | `src/gateway/security/policy_engine.rs` | Role-based access control |

---

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Tool System](TOOL_SYSTEM.md) - How bash_exec works
- [Gateway](GATEWAY.md) - Security RPC methods
