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

**Location**: `core/src/gateway/security/`, `core/src/exec/`

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

**Location**: `core/src/gateway/security/policy_engine.rs`

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

**Location**: `core/src/gateway/security/invitation_manager.rs`

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

**Location**: `core/src/gateway/session_manager.rs`

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

**Location**: `core/src/exec/kernel.rs`

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

**Location**: `core/src/exec/parser.rs`

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

**Location**: `core/src/exec/risk.rs`

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

**Location**: `core/src/exec/manager.rs`

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

**Location**: `core/src/exec/allowlist.rs`

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

**Location**: `core/src/exec/masker.rs`

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

**Location**: `core/src/permission/`

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

**Location**: `core/src/exec/storage.rs`

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

**Location**: `core/src/exec/ipc.rs`

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

## See Also

- [Architecture](ARCHITECTURE.md) - System overview
- [Tool System](TOOL_SYSTEM.md) - How bash_exec works
- [Gateway](GATEWAY.md) - Security RPC methods
