# Runtime Security Layer Enhancement Design

> Non-breaking enhancements to Aleph's runtime security, inspired by ClawShell's patterns.
> **Principle**: All enhancements are opt-in via configuration. Default behavior is unchanged.

## 1. Context & Goals

### 1.1 Current State

Aleph already has a solid security foundation:

- **Shell Security**: `ExecSecurityGate` with `SecurityKernel` (4-tier risk: Blocked/Danger/Caution/Safe), human approval for Danger, `SecretMasker` for output
- **Sandbox**: `WorkspaceSandbox` with OS-native isolation (macOS seatbelt, Linux bwrap, Windows AppContainer)
- **PII**: `PiiEngine` with 7 hardcoded rules, `PrivacyConfig` with per-platform overrides
- **Secrets**: `SecretVault` with AES-256-GCM, `{{secret:NAME}}` placeholders, `LeakDetector` with hash-based tracking
- **Audit**: `SecurityAuditLog` for security events

### 1.2 Known Gaps

| Gap | Location | Severity |
|-----|----------|----------|
| Sandbox routing TODO | `exec_security_gate.rs:189-190` | **Critical** — Danger-approved commands don't actually enter sandbox |
| PII rules are hardcoded | `pii/rules/mod.rs` | Medium — Can't add new PII types without code change |
| No PII scan on inbound responses | `runtime_guard.rs:process_inbound` | Medium — LLM could echo back user's PII |
| Log scrubbing stubbed | `shared/logging/pii_filter.rs` | Medium — Logs may contain plaintext PII |
| Leak patterns are hardcoded | `secrets/leak_detector.rs` | Low — Limited secret type coverage |
| No virtual key mapping | `secrets/injection.rs` | Low — Agent knows real secret names |

### 1.3 Design Principles

1. **Non-breaking**: All changes are backward-compatible. Existing configs work unchanged.
2. **Opt-in**: New features are disabled by default, enabled via `aleph.toml`.
3. **Fix first**: Priority is fixing the sandbox routing TODO.
4. **Enhance second**: Add configurability without removing existing hardcoded safety nets.

---

## 2. Shell Security Enhancements

### 2.1 Fix: Sandbox Routing (Critical)

**Problem**: `ExecSecurityGate.pre_execute()` returns `use_sandbox: bool`, but `SingleStepExecutor` ignores it.

**Fix**: Wire the sandbox routing in `SingleStepExecutor`.

```rust
// In SingleStepExecutor::execute_tool()
if ExecSecurityGate::is_exec_tool(tool_name) {
    let pre_decision = self.security_gate.pre_execute(tool_name, args, identity).await;
    match pre_decision {
        PreExecDecision::Allow { use_sandbox } => {
            if use_sandbox {
                // Route through WorkspaceSandbox
                self.execute_via_sandbox(tool_name, args).await
            } else {
                // Existing path (direct execution)
                self.execute_direct(tool_name, args).await
            }
        }
        PreExecDecision::Block { reason } => { /* existing */ }
    }
}
```

**Config**: No config needed — this is a bug fix, always enabled.

### 2.2 Enhancement: Configurable Risk Patterns

**New Config Section** (`aleph.toml`):

```toml
[security.shell]
# Enable custom patterns (default: false — uses built-in patterns only)
enable_custom_patterns = false

# Custom patterns are ADDITIVE to built-in patterns
# Built-in patterns are ALWAYS active as safety floor
[[security.shell.custom_blocked]]
pattern = "^dangerous_tool\\s+"
reason = "Custom blocked pattern"

[[security.shell.custom_danger]]
pattern = "^custom_admin_cmd\\s+"
reason = "Requires approval"

[[security.shell.custom_safe]]
pattern = "^my_safe_script\\s+"
reason = "Auto-approved"
```

**Implementation**:
- Add `ShellSecurityConfig` struct in `config/types/security.rs`
- Modify `SecurityKernel` to load custom patterns at init
- Custom patterns are additive — built-in patterns remain as safety floor
- If `enable_custom_patterns = false` (default), behavior is identical to today

---

## 3. PII Enhancements

### 3.1 Enhancement: Configurable PII Rules

**New Config Section** (`aleph.toml`):

```toml
[privacy]
# Existing fields unchanged...

# Enable configurable rules (default: false)
enable_configurable_rules = false

# Configurable rules are ADDITIVE to built-in rules
[[privacy.custom_rules]]
name = "ssn"
pattern = "\\b\\d{3}-\\d{2}-\\d{4}\\b"
placeholder = "[SSN]"
action = "block"  # block | warn | off
severity = "critical"  # low | medium | high | critical

[[privacy.custom_rules]]
name = "passport"
pattern = "\\b[A-Z]{1,2}\\d{6,9}\\b"
placeholder = "[PASSPORT]"
action = "block"
severity = "high"
```

**Implementation**:
- Add `ConfigurablePiiRule` struct implementing `PiiRule` trait
- Modify `PiiEngine::new()` to load custom rules if enabled
- Custom rules coexist with built-in rules, ordered by severity
- Default: `enable_configurable_rules = false` — only built-in rules active

### 3.2 Enhancement: Inbound Response PII Scanning

**New Config**:

```toml
[privacy]
# Scan LLM responses for PII before displaying to user (default: false)
scan_inbound_pii = false
```

**Implementation**:
- In `RuntimeSecurityGuard::process_inbound()`, add PII filtering step
- Current: only leak detection (secret patterns)
- Enhanced: if `scan_inbound_pii = true`, also run PII engine on response
- PII found in response is redacted with placeholders before display
- Default `false` — no change to existing behavior

### 3.3 Fix: Log Scrubbing Layer

**Problem**: `PiiScrubbingLayer::on_event` is a no-op.

**Fix**: Implement `on_event` to call `scrub_pii` on log messages.

```rust
impl<S> Layer<S> for PiiScrubbingLayer {
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        // Scrub PII from the event before forwarding
        // This is a simplified version — actual implementation
        // wraps the formatter to apply scrub_pii
    }
}
```

**Config**: No config — this is a bug fix. The scrubbing layer is already registered in logging setup; it just didn't do anything.

---

## 4. Secret Enhancements

### 4.1 Enhancement: Virtual Key Mapping

**New Config Section** (`aleph.toml`):

```toml
[secrets]
# Enable virtual key mapping (default: false)
enable_virtual_keys = false

# Virtual keys hide real secret names from the agent
# Agent sees "production_db", resolves to actual secret name
[[secrets.virtual_keys]]
virtual_name = "production_db"
real_secret_name = "db_password_production"

[[secrets.virtual_keys]]
virtual_name = "staging_api"
real_secret_name = "openai_api_key_staging"
```

**Usage**:
```
# Agent prompt uses virtual key
"Connect to {{secret:production_db}}"

# Resolves through mapping to real secret
# Then real secret is resolved from vault
```

**Implementation**:
- Add `VirtualKeyResolver` in `secrets/key_map.rs`
- Modify `extract_secret_refs` to optionally apply virtual key mapping
- Mapping is a pure name translation layer — doesn't change vault storage
- Default: `enable_virtual_keys = false` — agent sees real secret names (current behavior)

### 4.2 Enhancement: Configurable Leak Patterns

**New Config Section** (`aleph.toml`):

```toml
[secrets]
# Enable custom leak patterns (default: false)
enable_custom_leak_patterns = false

# Custom patterns are ADDITIVE to built-in patterns
[[secrets.custom_leak_patterns]]
name = "JWT Token"
pattern = "eyJ[a-zA-Z0-9_-]*\\.eyJ[a-zA-Z0-9_-]*\\.[a-zA-Z0-9_-]*"

[[secrets.custom_leak_patterns]]
name = "Database URL"
pattern = "postgres://[^:]+:[^@]+@"
```

**Implementation**:
- Modify `LeakDetector` to accept custom patterns at construction
- Patterns are additive — built-in patterns remain
- Default: `enable_custom_leak_patterns = false` — only built-in patterns active

---

## 5. Configuration Summary

All new config fields with defaults:

```toml
[security.shell]
enable_custom_patterns = false  # New

[privacy]
# Existing fields unchanged...
enable_configurable_rules = false  # New
scan_inbound_pii = false  # New

[secrets]
# Existing fields unchanged...
enable_virtual_keys = false  # New
enable_custom_leak_patterns = false  # New
```

**With all defaults**: Behavior is identical to today.

---

## 6. Implementation Plan

### Phase 1: Fixes (No Config Changes)
1. **Fix sandbox routing TODO** in `SingleStepExecutor`
2. **Fix log scrubbing** in `PiiScrubbingLayer`

### Phase 2: Configurable Shell Security
3. Add `ShellSecurityConfig` and config loading
4. Modify `SecurityKernel` for custom patterns

### Phase 3: Configurable PII
5. Add `ConfigurablePiiRule` and config loading
6. Add inbound PII scanning option

### Phase 4: Configurable Secrets
7. Add `VirtualKeyResolver`
8. Add custom leak patterns

### Phase 5: Testing & Cleanup
9. Unit tests for all new modules
10. Integration tests with config toggles
11. Verify backward compatibility

---

## 7. Files to Create/Modify

### New Files
- `src/config/types/security.rs` — ShellSecurityConfig
- `src/pii/configurable.rs` — ConfigurablePiiRule
- `src/secrets/key_map.rs` — VirtualKeyResolver

### Modified Files
- `src/executor/exec_security_gate.rs` — Fix sandbox routing
- `src/executor/single_step_executor.rs` — Wire sandbox flag
- `src/exec/kernel.rs` — Custom pattern support
- `src/pii/engine.rs` — Configurable rules, inbound scanning
- `src/pii/rules/mod.rs` — Load custom rules
- `src/secrets/leak_detector.rs` — Custom patterns
- `src/secrets/injection.rs` — Virtual key mapping
- `src/secrets/placeholder.rs` — Virtual key resolution
- `shared/logging/pii_filter.rs` — Fix on_event
- `src/config/types/privacy.rs` — New fields
- `src/config/types/secrets.rs` — New fields

---

## 8. Backward Compatibility

- All existing `aleph.toml` configs work unchanged
- All new fields have `Default` impl that preserves current behavior
- Hardcoded patterns remain as safety floor even when custom patterns enabled
- No changes to public APIs — only additive

---

## 9. Testing Strategy

- Unit tests for each new module
- Integration tests: verify behavior with all-default config matches pre-change
- Integration tests: verify new features work when enabled
- Regression tests: ensure existing security tests still pass

---

*Design written: 2026-04-27*
*Based on analysis of ClawShell's Runtime Security Layer and Aleph's existing architecture*
