# Security System

> Shell execution safety, approval workflows, and per-channel tool permissions

---

## Overview

Aleph's security system provides:
- **Permission Model**: single-tier Gateway-token auth — loopback is operator zero-config; a remote presents the shared token (or is walled). Authorized == full local-equivalent authority (see below)
- **Tool Permission Enforcement**: Per-channel tool permissions via `ScopedToolService` — governs *what an agent may do*, orthogonal to *who may connect*
- **Exec Tier**: the one user-facing dial over tool permissions — `Ask` / `Auto` / `Full`, metadata-driven, fail-closed for unknown tools (see below)
- **Exec Approval**: Human-in-the-loop, keyed on the *action* (the actual command / arguments), not the tool name
- **Command Analysis**: Static analysis of command risk
- **Allowlist/Blocklist**: Fine-grained command control
- **Output Masking**: Sensitive data protection

**Location**: `src/gateway/security/` (crypto + vault: `crypto.rs`, `shared_token.rs`, `store/`, `token_readonly.rs`), `src/tools/scoped/`, `src/config/types/policies/exec_tier.rs`, `src/sandbox/exec_approval/`, `src/exec/`

---

## Permission Model (network boundary + Gateway token)

The trust boundary is the network boundary, gated by a single shared
**Gateway token** (see [Trust model](#auth-ux)). The Panel connects over plain
WS (same-origin HTTP) — **not** the channel pipeline — so a thin-shell App
reaching a LAN core behaves exactly like a browser opening the core's IP:

- **Loopback** (the local desktop App / same machine): always authorized as
  **operator**, no token (single-machine zero-config).
- **Remote** (LAN): must present the shared Gateway token (`aleph-<uuid>`,
  provisioned at boot by `SharedTokenManager`) in the `connect` handshake.
  A valid token grants the **same** operator authority as local — there is no
  Chat/Config sub-tier. A missing / invalid token leaves the connection
  unauthorized behind a **login wall**.
- **Revocation**: rotate the token (`gateway.token.rotate`), which invalidates
  every previously authorized remote. No per-device sessions.

Two ways to present the token, both equivalent to a browser login:

- **Token box** — open the core IP, the Panel shows a token input; paste the
  token → authorized.
- **QR / link** — scan the QR (or open `http://<ip>:<port>/?token=<token>`)
  shown in **Settings → Security → Gateway token**; the token rides the URL.

Operators read the token via that Settings section or the
`aleph-server bootstrap-token` CLI.

### Enforcement

- **Login wall** (`server::handler` + `handlers::connect::connect_authorized`):
  the WS dispatch refuses every method except `connect` to an unauthorized
  connection. The handshake computes the verdict — loopback, or a valid token
  via `SharedTokenManager::validate` — stamps `ConnectionState.caller_role`
  (`operator` / `guest`), and echoes `role` / `authorized` / `needs_token` so
  the Panel renders the wall or unlocks the full app.
- **Channel tool gate** (`tools/scoped/dispatch.rs` +
  `method_authz::tool_requires_operator`): governs **channels only**. The
  inbound router stamps each channel run's `caller_role` from its
  `ChannelPermissionLevel` (default `Chat` ⇒ `guest`), and the tool-dispatch
  chokepoint refuses self-config tools to a chat-tier channel (e.g. a default
  Telegram bot). An authorized Panel is always operator, so this gate is a
  no-op for it — it is not a Panel sub-tier.

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

## Exec Tier (Ask / Auto / Full)

**Location**: `src/config/types/policies/exec_tier.rs` (rules + the single
precedence composition point), `src/tools/scoped/` (enforcement),
`src/gateway/execution_engine/turn_permissions.rs` (per-turn resolution).

The exec tier is the **one user-facing dial** over tool permissions. It is not a
second policy engine and not a second enforcement mechanism: it is a rule
consulted at the chokepoint every tool call already funnels through, whenever no
explicit `[policies.tool_permissions]` entry names the tool.

| Tier | What it asks about | Notes |
|------|--------------------|-------|
| `Ask` | every mutating / side-effecting tool | read-only tools stay allowed, so the model can still investigate |
| `Auto` *(default)* | the irreversible tail only | `*_delete`, `vault_*`, `team_disband`, an MCP server's `destructiveHint`, and `file_ops` `delete` / `move` (argument-level) |
| `Full` | nothing | the command-policy floor still applies |

### The lattice (who wins)

```
explicit [policies.tool_permissions] entry   (exact name > glob)
        ↓  (nothing named this tool)
configured `default`   TIGHTENED BY   the tier's verdict
        ↓  (restrictive_min — the tier can only raise, never widen)
[sandbox.command_policy] hardline floor      (no tier can lower it — not even Full)
```

`effective_permission(permissions, tier, facts)` is the **only** place this
precedence exists. Both consumers — `ScopedToolService::permission_for` (the
loop) and the gateway slash-command fast path — call it, so the two surfaces
cannot drift apart. Consulting the tier *before* the default (the pre-2026-07-14
shape) inverted a `default = "deny"` install into ask-by-default for exactly the
tools the tier meant to guard.

### The rules read declared metadata, never the tool's name

`ToolFacts { name, idempotent, requires_approval }` is filled from the tool's own
`ToolDefinition`:

- `idempotent` ← `LoopTool::is_idempotent()` — the builtin pure-read allowlist
  (`tools/retry.rs::IDEMPOTENT_BUILTIN_TOOLS`) or an MCP server's
  `readOnlyHint` / `idempotentHint` (`mcp/protocol.rs::is_idempotent`).
  Anything that declares nothing is `false`.
- `requires_approval` ← `ToolDefinitionMetadata` (an MCP server's
  `destructiveHint`).

Hence: **not idempotent = mutating**, and **an unknown tool is non-idempotent**,
so `Ask` is fail-closed for every tool Aleph has never heard of. A table of name
globs cannot do this — MCP tools register as `{server_id}__{tool}`
(`github__delete_repo`), and any glob table silently lets whole families through
the gate it claims to hold.

The one exception is argument-level: `file_ops` multiplexes `list` and `delete`
behind a single name, so `ExecTier::asks_for_arguments` reads the `operation`
field. That is a deterministic safety hard filter (explicitly permitted by R7),
not a judgement about intent.

### Where the tier comes from, per turn

`resolve_turn_permissions` resolves it once per run: **request > session >
global**.

- **request** — the Panel composer pill sends the tier *with* the first message
  (`chat.send`), so it governs the very turn it was armed for.
- **session** — persisted to `identity_meta.custom["exec_tier"]` via
  `sessions.patch` (the same carrier as `project_root`). `null` = follow global.
  `sessions.patch` validates the value against `ExecTier::from_id`, exactly as
  `chat.send` does.
- **global** — `[policies] exec_tier` in config, read live per turn (no restart).
- A **non-operator** caller (a chat-tier channel) is clamped after resolution: it
  can tighten the tier but never raise it.

### The three bypasses that were closed (do not re-open them)

Every one of these was a surface that could execute a tool **without passing
through `ScopedToolService`**. When you add a new such surface, this is the
question to ask first.

1. **Slash-command fast path** (`execution_engine/slash_command.rs`). `/bash`,
   `/file_ops` etc. dispatched straight into `BuiltinToolRegistry` — no tier, no
   `tool_permissions`, no `requires_confirmation`, no operator gate — and the
   path is reachable **from channels**. Now `slash_gate_reason()` returns the
   existing `ExecutionError::Fallthrough` for any gated call, routing it into the
   fully-gated agent loop; ungated slash commands keep their deterministic fast
   path.
2. **`tools.invoke` RPC** (`gateway/handlers/tools_invoke.rs`). Its denylist
   (`security/dangerous_tools.rs`) named 7 tools that **do not exist in this
   repo** — it had been inert its entire life. It now names the real ones and
   also refuses confirmation-gated tools (`is_confirmation_gated`, reading
   `CONFIRMATION_REQUIRED_TOOLS`). *Known limitation*: argument-level asks (an
   `Auto`-tier `file_ops delete`) are still ungated on this direct-invoke
   surface — closing that needs an approval transport for the RPC.
3. **Background runs** (goal / loop continuations, cron, heartbeat, a2a). A
   continuation used to drop both `caller_role` and the channel's
   `tool_permissions` layer — a chat-tier Telegram session escalated itself to
   local-operator authority by continuing. `carry_policy_metadata` now forwards
   exactly those two keys (and deliberately *not* `channel_id` /
   `conversation_id`, which would make an unattended run's approvals look
   deliverable).

### Unattended = fail closed

A run with no human attached is stamped `UNATTENDED_KEY`
(`execution_engine/mod.rs`) at every headless producer: cron **when its approval
is not routable** (a job carrying both a source channel and a conversation has a
real `/approve` path — stamping it would auto-deny a working HITL flow),
heartbeat and a2a always. `continuation_metadata` inserts the marker **last**, so
an inherited key can never demote a continuation to attended.

`ScopedToolService` then denies confirm-gated tools immediately instead of
publishing an approval card into the void and blocking for the 120s timeout. The
model is told the run is unattended.

Teams (dispatcher / broadcast) are deliberately **not** stamped: a member run's
approvals resolve to a Panel card, and the user who dispatched the team is the
operator watching it.

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

**Location**: `src/exec/manager.rs` (`ExecApprovalManager` — pending/resolve
pairing), `src/sandbox/exec_approval/` (the gate: `gate.rs`, `action.rs`,
`session_memory.rs`, `denial_ledger.rs`).

The gate is **action-aware**: it is asked to approve *this call*, not *this tool*.

```rust
// src/sandbox/exec_approval/action.rs
pub struct ApprovalAction {
    pub tool_name: String,
    pub summary: String,             // the ACTUAL call, redacted + capped
    pub cwd: Option<String>,
    pub analysis: Option<CommandAnalysis>,
    pub reason: String,
}

// src/sandbox/exec_approval/gate.rs
trait ApprovalRequester {
    async fn request_approval(&self, action: &ApprovalAction) -> ApprovalOutcome;
}
```

`summary` renders what will actually happen — the command for `bash` /
`code_exec` (fed through the real `exec::parser::analyze_shell_command`),
`operation=delete path=…` for `file_ops`, `k=v` otherwise — then passes through
`SecretMasker`, is newline-flattened, and is capped at 200 chars on a
`char_indices()` boundary. **A confirmation gate that hides what it is gating
trains the user to click Approve**, which converts the whole tier system into
theater; every surface (Panel card, plain-text channel prompt, cluster node over
reverse RPC) now shows the action.

### Grants key on the action, not the tool

`confirm_with_memory` computes `grant_fingerprint(tool, &args)` once — over the
**raw canonical arguments**, not the redacted summary (redaction collapses
distinct secrets to one placeholder, so a grant on one credential would cover
another) — and uses it for **both** the session memory and the denial ledger.

Consequence: "Allow for this session" authorizes **that exact call**. Under
`Ask`, `bash` re-prompts per distinct command instead of whitelisting arbitrary
argv after one approval (codex's `ApprovalStore` semantics). Noisier, deliberately.

### Approval decision

```rust
// src/exec/socket.rs
pub enum ApprovalDecisionType {
    AllowOnce,    // this call
    AllowSession, // this call, for the rest of the session (by fingerprint)
    AllowAlways,  // clamped to AllowSession by clamp_decision — no surface offers it
    Deny,
}
```

- **Timeout ⇒ refusal.** `DEFAULT_APPROVAL_TIMEOUT_MS` = 120s;
  `ApprovalOutcome::is_approved` excludes `Timeout`. A timeout is deliberately
  **not** written to the denial ledger — an expired card is not a decision.
  There is no fail-open path.
- **Orphans cannot hijack a card.** `PendingEntry::is_live()` (not expired ∧
  receiver not closed) filters both `resolve_for_session` and `list_pending`, so
  a cancelled run's zombie can no longer win the `/approve` FIFO or render.
- **Denial is terminal** for that call, and is returned to the model as an
  in-context instruction not to retry it, rewrite it, or achieve the same result
  by other means. Three denials trip the sticky pause in `denial_ledger.rs`.

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

There is no `src/permission/` module and there never was one. Limiting *what an
agent may do* is exactly three things, and they compose in this order:

1. **`[policies.tool_permissions]`** — per-tool `allow` / `ask` / `deny`, exact
   name or glob, merged global → agent → channel, most-restrictive wins
   (`src/config/types/policies/tool_permissions.rs`).
2. **The exec tier** — `Ask` / `Auto` / `Full`, which tightens the configured
   `default` for tools nobody named (see [Exec Tier](#exec-tier-ask--auto--full)).
3. **`[sandbox.command_policy]`** — the hardline command floor, which no tier and
   no permission entry can lower (`src/sandbox/command_policy/`).

All three are enforced at one chokepoint, `src/tools/scoped/` — `Deny` hides the
tool from the model *and* refuses the call; `Ask` routes to the approval gate.

The action-type approval engine (`src/approval/`) is a separate, older axis over
*capability domains* (desktop / browser / system / pim / automation), not tool
names; its `Ask` hands `approval_required` back to the LLM rather than running a
deterministic HITL gate (R7/R9).

---

## Audit Logging

**The live trail is the session event log.** Every approval decision at the
enforcement chokepoint is recorded by
`tools/scoped/dispatch.rs::record_approval_decision`, which writes
`ToolCallApproved` / `ToolCallDenied` session events. Query it through the
session service, alongside every other event of the run that produced it.

**Deleted (2026-07-14), do not go looking for it**: `src/exec/approval/storage.rs`
+ `audit.rs` and the `aleph-server audit` CLI command. They queried three SQLite
tables (`~/.aleph/approval_audit.db`) whose **only writers were test helpers** —
an operator running `audit` got zeros and concluded nothing had happened, while
the real trail sat in the session event log. That is worse than dead code.

Deleted in the same sweep, all zero-consumer:
`src/exec/approval/{escalation,binding,path_canonicalize}.rs` (path-escalation,
binding-compliance and sensitive-directory checks that nothing ever called),
`benches/approval_performance.rs`, and
`exec/allowed_decisions.rs::{decisions_for_risk, assess_command_decisions,
risk_segments}`.

Security *events* (not approvals) still log through
`src/security/audit.rs`; SSRF has its own trail (see below).

---

## IPC Security

**Location**: `src/exec/bridge.rs` + `src/exec/socket.rs`

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

## Trust model: network boundary + Gateway token {#auth-ux}

The trust boundary is the network boundary, gated by a single shared
**Gateway token**. Loopback is the implicit operator (zero-config); a remote
connection must present the token at the `connect` handshake
(`src/gateway/handlers/connect.rs::connect_authorized`). A valid token = full
operator authority (single tier, identical to local); a missing / invalid one
is walled (the WS dispatch refuses every method but `connect`). Revocation is
token rotation (`gateway.token.rotate`).

### Network boundary = reachability

- **Default — loopback only.** `aleph-server` binds `127.0.0.1`
  (`GatewayServerConfig::default`), so only processes on the same machine
  can connect. A single-machine desktop install needs zero configuration and
  is auto-authorized as operator.
- **LAN opt-in.** Set `[gateway] host = "0.0.0.0"` in
  `~/.aleph/config.toml` to listen on every interface. A remote device on the
  LAN can then reach the socket, but is **walled until it presents the Gateway
  token** — so exposure of the socket no longer equals control of the agent.
  Still, treat the token as the single key to everything (an authorized remote
  has full operator authority, including PTY / shell): share it only over a
  trusted channel, and rotate it if it may have leaked.
- **Beyond the LAN.** To reach Aleph across the internet, front it with
  your own reverse proxy / VPN / SSH tunnel that terminates trust
  *upstream* of the gateway. The Gateway token is the only transport auth
  Aleph itself provides.

### The one remaining guardrail: WS Origin check

Browsers attach an unforgeable `Origin` header to every WebSocket upgrade
and cross-origin `fetch`. A malicious public web page the user happens to
visit can still *reach* `ws://127.0.0.1:18790/ws` (loopback is not
firewalled from the browser), so without an origin check it could open a
control channel to the local daemon — the cross-origin-WebSocket
confused-deputy (and its DNS-rebinding variant). The origin gate
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
| **same-origin** (Origin authority == request `Host`) | allow **only if the `Host` is an IP literal or loopback** | LAN deployments reached by IP (`http://10.10.10.6:18790`) work without config; an IP literal cannot be DNS-rebound. A **domain** `Host` is *not* auto-allowed — the deployment must add its origin to `allowed_origins` (see DNS-rebinding note below). |
| anything else (public web domain) | **deny** | |

Note the gate does **not** auto-allow arbitrary private-LAN IPs by range —
a cross-origin browser request from another LAN host is only accepted when
it is same-origin with the gateway's `Host`, in the allow-list, or
loopback/`tauri:`. Native LAN clients carry no `Origin` and pass freely.

> **DNS-rebinding — defended.** A classic DNS-rebinding attack rebinds a
> domain (`evil.com`) to the gateway's own address so the victim's page
> carries `Origin == Host == evil.com` and would slip past a naive
> same-origin check. Aleph closes this by gating the same-origin branch on
> the `Host`: same-origin is auto-allowed **only when the `Host` is an IP
> literal or loopback** (`127.0.0.0/8`, `::1`, `localhost`, `*.localhost`, or
> a bare IPv4/IPv6 address). A rebinding attack must use a domain name (the A
> record is what gets rebound), and a domain `Host` no longer passes
> same-origin — it falls through to deny. The trade-off: a zero-config
> **domain** deployment (serving the panel from `aleph.example.com` with no
> `allowed_origins`) is now rejected and must add its origin to
> `[gateway] allowed_origins`. LAN deployments reached by IP and loopback
> access are unaffected.

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

## Gap analysis: exec tiers vs codex / hermes-agent / pi {#exec-tier-gap-analysis}

> Same convention as the **openclaw 对照映射表** in [CLUSTER.md](CLUSTER.md):
> per-dimension verdicts, anchored to real code. **Read this before re-comparing
> Aleph's permission model against another agent's — the comparison has been
> done.** Verdicts: **aligned** (same shape, no action) · **aleph-superior**
> (keep, protect by test) · **gap** (closed in the 2026-07-14 round unless said
> otherwise) · **deliberately-not-ported** (do not "helpfully" add it).

| Dimension | codex | hermes / pi | Aleph | Verdict |
|---|---|---|---|---|
| Policy axes | 2 live axes: `AskForApproval` × `PermissionProfile` | hermes: `approvals.mode {manual\|smart\|off}` × gate stack. pi: declined a sandbox entirely | 1 axis: `ExecTier {Ask,Auto,Full}`, orthogonal to `[sandbox.command_policy]` (an undisableable floor) | **aligned** |
| Tier semantics | 4 approval values, but 2 of the 3 shipped presets share one — the axis that really moves is the sandbox profile | hermes: 3 modes + a HARDLINE floor under the top one. pi: 3-valued project TRUST, gating config *loading* | 3 tiers with honest semantics; the floor holds under all three | **aligned** |
| Gate input | name-based argv allowlist + a Starlark rules engine over the command string | hermes: 47 regex `DANGEROUS_PATTERNS` over argument content. pi: tools declare *no* risk metadata; every gate re-derives danger from regex | **metadata-driven**: `ToolFacts` read off the declared `ToolDefinition`, never the name; one argument-level filter (`file_ops`) | **aleph-superior** — do not regress toward regex-on-shell-strings |
| Safe-read bypass | `is_known_safe_command` argv allowlist, compositional over `&& \|\| ; \|` | hermes: permanent glob `command_allowlist`. pi: patterns exist only in an example | `IDEMPOTENT_BUILTIN_TOOLS` (pure-read builtins) + MCP `readOnlyHint`; default-deny for anything unlisted | **aligned** |
| Memory key | `{env, CANONICALIZED argv, cwd, sandbox perms}` | hermes: keys on a *pattern*, so "always" on `rm -r*` allowlists every future one. pi: no memoization at all | `grant_fingerprint(tool, canonical args)`, shared by session memory **and** the denial ledger | **gap → closed** (was: the bare tool name) |
| Escalation / retry ladder | model-declared `SandboxPermissions` + **`ToolOrchestrator`'s harness-side retry ladder**: on `SandboxErr::Denied`, pick a recovery strategy and re-run | hermes: plugin may re-escalate into the same gate. pi: "approve-with-modification" mutates `event.input` in place, with no re-validation | Neither. Denial is terminal and is returned to the model as an instruction not to retry | **deliberately-not-ported** — see below |
| Timeout / orphans | approvals block forever; turn death drops the sender → fails closed | hermes: timeout ⇒ deny ("silence is not consent"). pi: `select` timeout returns `undefined`, so a gate written `if choice === "No"` **fails open** | 120s ⇒ refusal everywhere; timeout is *not* ledgered (an expired card is not a decision); `is_live()` evicts orphan waiters | **aleph-superior** |
| Runtime switching UX | `/permissions` → 3 presets, session-scoped, admin-lockable | hermes: `/yolo` per session; shift-click writes it globally. pi: restrictions persist to the session transcript and survive resume | composer pill (per-session, rides the first message) + Settings→Policies (global, live per turn) | **gap → closed** (Panel lost the display on select/reload while the server kept enforcing) |
| Unknown-tool default | MCP: an unannotated tool requires approval | hermes: documented fail-open hole in headless non-gateway contexts. pi: no metadata, so "unknown" is meaningless | fail-closed by construction: unknown ⇒ non-idempotent ⇒ mutating ⇒ `Ask` holds | **aligned** — protect by test |
| Per-tool override | per-server *and* per-tool approval mode; two-tier memory | hermes: `approvals.deny` globs that survive yolo; per-rule grain | `[policies.tool_permissions]` exact + glob, 3-tier merge, most-restrictive-wins; explicit beats the tier | **gap → closed** (the tier used to *widen* a `default = "deny"`) |
| Background inheritance | approval store lives on shared session services | hermes: background writes must stage; **cron gets its own axis** because "ask" is meaningless with no human attached | subagents inherit correctly; continuations now carry `caller_role` + channel permissions; headless producers stamp `unattended` ⇒ fail closed | **gap → closed** |
| Audit trail | telemetry on the orchestrator path | hermes: observability hooks, everything redacted before display | live: `ToolCallApproved` / `ToolCallDenied` session events | **gap → closed** by deleting the dead SQLite trail that reported zeros |
| Floor beneath the top tier | under `Never`, dangerous commands are Forbidden — **but only when the sandbox profile is Managed**; with it off, the top tier is unbounded | hermes: `HARDLINE_PATTERNS` + a user-editable `approvals.deny` floor that survives yolo | `[sandbox.command_policy]` holds under every tier including `Full` (unit-pinned); a `deny` override also beats the tier | **aligned** — better placed than codex's |
| What the human SEES | full argv + cwd + the model's own justification | hermes: the whole command, redacted, with all findings merged into one prompt. pi: typed per-tool event | the redacted **action summary** (the command / `operation=delete path=…`), on every surface | **gap → closed** — this was the sharpest defect of round 1 |

### Deliberately not ported (do not add these)

- **codex's sandbox-escalation retry ladder.** On a sandbox denial, codex's
  `ToolOrchestrator` selects a recovery strategy and re-runs with elevated
  permissions. That is precisely R10's 5th 不 (不做错误恢复策略选择) and precisely
  what clause **A2** forbids: *let the model see and heal the error = yes; let the
  harness pick the recovery strategy for it = no.* Aleph compresses the denial
  into context and the model re-plans. A future round that "helpfully" adds a
  retry matrix is reverting an architectural decision, not fixing an omission.
- **codex's model-declared escalation flag** is R7-compatible in principle (the
  harness only checks a declared flag, it never classifies), but Aleph has no
  sandbox-escalation axis for it to target — it would be a zero-consumer
  abstraction (P6).
- **pi's approve-with-modification** (the gate mutates `event.input` in place).
  Tempting, but pi does **no re-validation after mutation**, and Aleph has no
  consumer for a third "allow-if-rewritten" state that a tier enum cannot express.

### Still open (honest)

- `tools.invoke` has no argument-level tier parity (needs an approval transport
  on the RPC surface).
- `sessions_send` sub-agent runs do not inherit `unattended` from a headless
  parent — `TurnContext` has no such field, so closing it is its own slice.
- No user-editable floor under `Full` in hermes' sense (an `approvals.deny` glob
  that survives yolo). `[policies.tool_permissions]` `deny` overrides already
  cover ~80% of it, since an explicit entry beats the tier.
- Free-text `/deny <reason>` back to the model (hermes has it; cheap, genuinely
  better than a bare "denied").

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
| Tool Permissions | `src/tools/scoped/` | Per-channel tool permission merge + the enforcement chokepoint |
| Exec Tier | `src/config/types/policies/exec_tier.rs` | Ask / Auto / Full — the rules and the one precedence composition point |
| Approval Gate | `src/sandbox/exec_approval/` | Action-aware confirmation, grant fingerprint, denial ledger |
| Gateway-surface denylist | `src/security/dangerous_tools.rs` | What `tools.invoke` refuses outright |
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
- [Feature Locator](FEATURE_LOCATOR.md) - §5.2 permission hierarchy, §5.3 approval gate, §5.12 exec tier (code anchors + what was deleted)
- [Tool System](TOOL_SYSTEM.md) - How bash_exec works
- [Gateway](GATEWAY.md) - Security RPC methods
- [Desktop Bridge](DESKTOP_BRIDGE.md) - Bridge isolation invariants
