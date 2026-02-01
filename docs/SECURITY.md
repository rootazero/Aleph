# Security System

> Shell execution safety, approval workflows, and permission management

---

## Overview

Aether's security system provides:
- **Exec Approval**: Human-in-the-loop for shell commands
- **Command Analysis**: Static analysis of command risk
- **Allowlist/Blocklist**: Fine-grained command control
- **Permission Rules**: Role-based access control
- **Output Masking**: Sensitive data protection

**Location**: `core/src/exec/`, `core/src/permission/`

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
file://~/.aether/*          # Aether config files
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
          "resource": "file://~/.aether/*",
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
