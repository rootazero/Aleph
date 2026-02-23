# Exec Security Integration Design

> **Status:** Approved for Implementation
> **Date:** 2026-02-23
> **Context:** Gap analysis between security planning docs and actual implementation

---

## Background

A comprehensive gap analysis revealed that Aleph's security infrastructure components
(SecurityKernel, ExecApprovalManager, SecretMasker, Sandbox) are fully implemented as
isolated modules but are **never wired into the actual bash/shell execution path**.

`code_exec.rs` / `bash_exec.rs` currently bypass the entire security infrastructure:
- Uses a local 8-pattern `BLOCKED_PATTERNS` array instead of `SecurityKernel`
- No human-in-the-loop approval for Danger-tier commands
- No `SecretMasker` applied to output
- No sandbox isolation
- `ExecApprovalManager` exists but is unreachable from tool execution

---

## Design Goal

Connect existing security components to the shell execution path using a
**three-layer defensive gate** inserted into `SingleStepExecutor`, consistent with
the existing `PolicyEngine` check pattern.

---

## Architecture

```
SingleStepExecutor::execute(action, identity)
 ├─ [Layer 3 · existing] PolicyEngine::check_tool_permission()
 ├─ [Layer 4 · NEW] ExecSecurityGate (only for bash/code_exec tools)
 │     ├─ SecurityKernel::assess(cmd) → Blocked/Danger/Caution/Safe
 │     ├─ Blocked  → immediate reject with reason
 │     ├─ Danger   → ExecApprovalManager::wait_for_approval(2min timeout)
 │     │             ├─ AllowOnce / AllowAlways → proceed
 │     │             └─ Deny / Timeout → reject
 │     └─ Safe/Caution → SandboxManager (macOS: sandbox-exec; others: WarnAndExecute)
 ├─ [existing] execute_tool_call(tool_name, args)
 └─ [Layer 5 · NEW] SecretMasker::mask(ToolSuccess.output)
```

---

## Risk Level Routing

| SecurityKernel Level | Action | Result |
|---------------------|--------|--------|
| **Blocked** | Immediate reject | `ToolError { "Blocked: ..." }` |
| **Danger** | Suspend + push approval request via ExecApprovalManager | Continue if approved; error if denied/timeout |
| **Caution** | Execute via SandboxManager (macOS: sandbox-exec; others: direct + warn) | Normal output |
| **Safe** | Execute via SandboxManager (lightweight isolation) | Normal output |
| **All successful outputs** | SecretMasker filters stdout/stderr in ActionResult | Redacted output |

---

## New Component: ExecSecurityGate

**Location:** `core/src/executor/exec_security_gate.rs`

```rust
pub struct ExecSecurityGate {
    security_kernel: SecurityKernel,             // stateless
    approval_manager: Arc<ExecApprovalManager>,  // shared
    sandbox_manager: Option<Arc<SandboxManager>>, // optional (macOS only currently)
    masker: SecretMasker,                        // stateless
}

pub enum PreExecDecision {
    Allow { use_sandbox: bool },
    Block { reason: String },
}

impl ExecSecurityGate {
    /// Check if tool is a shell execution tool requiring security gate
    pub fn is_exec_tool(tool_name: &str) -> bool {
        matches!(tool_name, "bash" | "code_exec")
    }

    /// Extract command string from tool arguments
    pub fn extract_command(tool_name: &str, args: &Value) -> Option<String> {
        match tool_name {
            "bash" => args["cmd"].as_str().map(String::from),
            "code_exec" => {
                // Only apply to shell language
                if args["language"].as_str() == Some("shell") {
                    args["code"].as_str().map(String::from)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Pre-execution gate: risk assessment + approval
    pub async fn pre_execute(
        &self,
        tool_name: &str,
        args: &Value,
        identity: &IdentityContext,
    ) -> PreExecDecision;

    /// Post-execution: mask secrets in output
    pub fn post_execute(&self, result: ActionResult) -> ActionResult;
}
```

### Approval Flow Detail

For **Danger**-tier commands:
1. Parse command via `analyze_shell_command()`
2. Build `ApprovalRequest` with `session_key` from `IdentityContext`
3. Call `approval_manager.create(request, DEFAULT_APPROVAL_TIMEOUT_MS)`
4. Broadcast approval request via `ExecApprovalForwarder` (Telegram/Discord/CLI)
5. `await approval_manager.wait_for_decision(record)` (2-minute timeout)
6. `AllowAlways` → additionally calls `add_to_allowlist(agent_id, executable)`

### Sandbox Strategy

- **macOS**: Use `MacOSSandbox` with `TempWorkspace` + `NetworkCapability::Deny` for Safe/Caution
- **Linux/Windows**: `FallbackPolicy::WarnAndExecute` (log warning, execute directly)
- **Blocked/Danger after approval**: Bypass sandbox (user explicitly approved)

---

## Modified Component: SingleStepExecutor

**Location:** `core/src/executor/single_step.rs`

Add optional `exec_security_gate` field and builder method:

```rust
pub struct SingleStepExecutor<R: ToolRegistry> {
    // existing fields...
    exec_security_gate: Option<Arc<ExecSecurityGate>>, // NEW
}

impl<R: ToolRegistry> SingleStepExecutor<R> {
    /// Attach security gate for shell execution
    pub fn with_exec_security_gate(mut self, gate: Arc<ExecSecurityGate>) -> Self {
        self.exec_security_gate = Some(gate);
        self
    }
}
```

Modified `execute()` flow (pseudocode):

```rust
// After PolicyEngine check, before execute_tool_call:
if let Some(gate) = &self.exec_security_gate {
    if ExecSecurityGate::is_exec_tool(&normalized_tool_name) {
        match gate.pre_execute(&normalized_tool_name, arguments, identity).await {
            PreExecDecision::Block { reason } => return ActionResult::ToolError { ... },
            PreExecDecision::Allow { .. } => { /* proceed */ }
        }
    }
}

let result = self.execute_tool_call(tool_name, arguments.clone()).await;

// After execute_tool_call:
if let Some(gate) = &self.exec_security_gate {
    if ExecSecurityGate::is_exec_tool(&normalized_tool_name) {
        return gate.post_execute(result);
    }
}
result
```

---

## Other Changes

### Sandbox Module Activation

**File:** `core/src/exec/sandbox/mod.rs`

Uncomment all `pub use` re-exports. Verify compilation passes.
No functional changes required — the implementations are complete.

### code_exec.rs Cleanup

**File:** `core/src/builtin_tools/code_exec.rs`

Remove the local `BLOCKED_PATTERNS` array and `is_code_blocked()` function.
Security enforcement is now handled by `ExecSecurityGate` at the executor layer.
Keep: runtime checks, timeout, output size limits, env filtering.

### CLI guests Completion

**File:** `apps/cli/src/commands/guests.rs`

Add missing subcommands from design doc:

```rust
#[derive(Subcommand)]
pub enum GuestsAction {
    Invite { ... },  // existing
    List { ... },    // existing
    Revoke { guest_id: String, #[arg(short, long)] force: bool }, // NEW
    Info { guest_id: String, #[arg(short, long, default_value = "text")] format: OutputFormat }, // NEW
}
```

Map to existing RPC methods:
- `Revoke` → `guests.revokeInvitation`
- `Info` → `guests.getActivityLogs` (filtered by guest_id)

---

## Integration Point: Gateway Initialization

The `ExecApprovalManager` is a shared service. It should be initialized at gateway
startup and passed to `SingleStepExecutor` via the builder:

```rust
// In gateway/server initialization (poe/worker.rs or gateway startup):
let approval_manager = Arc::new(ExecApprovalManager::new(storage));
let sandbox_manager = {
    #[cfg(target_os = "macos")]
    { Some(Arc::new(SandboxManager::new(Arc::new(MacOSSandbox::new())))) }
    #[cfg(not(target_os = "macos"))]
    { None }
};
let exec_gate = Arc::new(ExecSecurityGate::new(approval_manager, sandbox_manager));

let executor = Arc::new(
    SingleStepExecutor::new(tool_registry)
        .with_exec_security_gate(exec_gate)
);
```

---

## Files to Change

| File | Change Type | Effort |
|------|-------------|--------|
| `executor/exec_security_gate.rs` | Create | Medium |
| `executor/single_step.rs` | Modify (inject gate) | Small |
| `executor/mod.rs` | Modify (add export) | Minimal |
| `exec/sandbox/mod.rs` | Modify (uncomment re-exports) | Minimal |
| `builtin_tools/code_exec.rs` | Modify (remove BLOCKED_PATTERNS) | Small |
| `cli/src/commands/guests.rs` | Modify (add revoke/info) | Small |
| `poe/worker.rs` (or gateway init) | Modify (wire ExecSecurityGate) | Small |

---

## Security Guarantees After Implementation

1. **No bypass path** — every shell command goes through SecurityKernel before execution
2. **Human-in-the-loop** — Danger-tier commands require explicit user approval
3. **Frozen permissions** — approval uses `IdentityContext.session_key` for audit trail
4. **Output sanitization** — secrets in stdout/stderr are redacted before reaching LLM/user
5. **Sandbox isolation** — Safe/Caution commands run in OS-native sandbox on macOS
6. **Allow-always persistence** — approved patterns added to allowlist for future auto-approval

---

## What This Design Does NOT Cover (Future Work)

- Linux/Windows sandbox implementation (currently WarnAndExecute fallback)
- Data isolation (Memory namespace filtering per guest)
- Session history isolation (guest cannot read owner's history)
- Tool-level argument filtering (e.g., restrict `file:read` to specific paths)
- Persistent guest sessions (currently in-memory DashMap)
