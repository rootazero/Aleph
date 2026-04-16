# Runtime Security Orchestrator for Aleph — Phase 1 Design

> Date: 2026-04-16  
> Scope: Phase 1 of a 3-phase roadmap to harden Aleph's runtime security layer, inspired by clamshell's safety-belt architecture and aligned with Aleph's Rust-first design principles.

---

## 1. Goal and Scope

### Goal
Introduce a unified **Runtime Security Orchestrator** (`RuntimeSecurityGuard`) into Aleph's agent loop. It coordinates all existing security subsystems — secret injection, PII filtering, content sanitization, and leak detection — into a single, ordered, auditable pipeline that executes on every outbound (to LLM) and inbound (from LLM) message.

### Scope
- **New file**: `src/security/runtime_guard.rs` — the orchestrator core.
- **Minor edits**: `src/security/mod.rs`, `src/agents/run.rs`, `src/secrets/mod.rs` (visibility), `tests/security_integration.rs`.
- **Non-invasive**: We do **not** modify the internals of `src/secrets/vault.rs`, `src/pii/engine.rs`, `src/security/content_sanitizer.rs`, or `src/exec/leak_detector.rs`. We only call their existing public APIs.

### Out of Scope
- Vault encryption logic changes.
- New PII rule additions.
- Content sanitizer boundary-format changes.
- New persistent storage schemas.

---

## 2. Architecture Position

```
┌─────────────────────────────────────────────────────────────────┐
│                        Agent Loop                               │
│  ┌────────────┐    ┌─────────────────────┐    ┌────────────┐  │
│  │  outbound  │───▶│  RuntimeSecurityGuard│───▶│  LLM API   │  │
│  │  (prompt)  │    │  (orchestrate)      │    │            │  │
│  └────────────┘    └─────────────────────┘    └────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │  Security Pipeline (strict order)                         │  │
│  │  1. Extract & Resolve ──▶ find {{secret:NAME}}, resolve   │  │
│  │  2. Leak Detector     ──▶ scan before replacement          │  │
│  │  3. PII Filter        ──▶ detect & mask PII               │  │
│  │  4. Content Sanitizer ──▶ wrap external content           │  │
│  │  5. Replace Secrets   ──▶ inject resolved values          │  │
│  └──────────────────────────────────────────────────────────┘  │
│                           │                                      │
│                           ▼                                      │
│  ┌────────────┐    ┌─────────────────────┐    ┌────────────┐  │
│  │  inbound   │◀───│  RuntimeSecurityGuard│◀───│  LLM resp  │  │
│  │  (response)│    │  (scan response)    │    │            │  │
│  └────────────┘    └─────────────────────┘    └────────────┘  │
└─────────────────────────────────────────────────────────────────┘
```

---

## 3. Core Components

### 3.1 `RuntimeSecurityGuard`

**File**: `src/security/runtime_guard.rs`

```rust
/// Central orchestrator for runtime security checks.
pub struct RuntimeSecurityGuard {
    config: SecurityGuardConfig,
    pii_engine: Option<Arc<RwLock<pii::engine::PiiEngine>>>,
    leak_detector: Arc<Mutex<exec::leak_detector::LeakDetector>>,
    secret_leak_detector: Arc<Mutex<secrets::leak_detector::LeakDetector>>,
}

/// Configuration for the guard.
#[derive(Debug, Clone)]
pub struct SecurityGuardConfig {
    pub pii_filtering: bool,
    pub content_sanitization: bool,
    pub leak_detection: bool,
    pub secret_injection: bool,
    pub default_action_on_leak: LeakAction,
}
```

**Rationale for `Arc<Mutex<T>>`**:
- `LeakDetector` maintains injection state (`register_injected`) and is therefore stateful.
- The agent loop is async; we need `Send + Sync`.
- This pattern matches Aleph's existing sync-primitive conventions (`crate::sync_primitives::{Arc, Mutex, RwLock}`).

### 3.2 Pipeline Stages

```rust
/// Stages of the outbound security pipeline.
pub enum PipelineStage {
    SecretInjection,
    PiiFiltering,
    ContentSanitization,
    LeakDetection,
}

/// Result of processing content through the guard.
#[derive(Debug, Clone)]
pub enum GuardResult {
    /// Content is clean, proceed as-is.
    Clean { text: String },
    /// Content was redacted but safe to proceed.
    Redacted { text: String, reasons: Vec<String> },
    /// Content should be blocked.
    Blocked { reason: String, redacted_text: Option<String> },
    /// Content allowed but warnings emitted.
    Warned { text: String, warnings: Vec<String> },
}
```

### 3.3 Main Methods

```rust
impl RuntimeSecurityGuard {
    /// Process outbound content before sending to LLM.
    pub async fn process_outbound(
        &self,
        text: &str,
        context: SecurityContext,
    ) -> Result<GuardResult, SecurityGuardError>;

    /// Process inbound content received from LLM.
    pub fn process_inbound(&self, text: &str) -> Result<GuardResult, SecurityGuardError>;
}
```

### 3.4 `SecurityContext`

Carries request-level security metadata:

```rust
pub struct SecurityContext {
    /// True if the text contains external content requiring wrapping.
    pub has_external_content: bool,
    /// Source for the sanitizer when wrapping.
    pub external_source: Option<ContentSource>,
    /// Provider name for PII provider-exclusion checks.
    pub provider_name: Option<String>,
    /// Secrets injected during this request (tracked for leak detection).
    pub injected_secrets: Vec<InjectedSecret>,
}
```

---

## 4. Data Flow and Stage Order

### Outbound Pipeline (strict sequence)

1. **Placeholder Extraction & Secret Resolution**
   - Extract `{{secret:NAME}}` placeholders via `secrets::placeholder::extract_secret_refs`.
   - Resolve each secret via `AsyncSecretResolver`.
   - Build `InjectedSecret` records and register them with `secret_leak_detector` for downstream inbound tracking.
   - **Do NOT replace text yet** — this prevents leak detectors from flagging intentionally injected secrets.

2. **Leak Detection**
   - Call `exec::leak_detector::LeakDetector::scan_outbound` on the text that still contains placeholders.
   - Call `secrets::leak_detector::LeakDetector::scan_outbound` on the same text.
   - If leaks are found, apply `default_action_on_leak`:
     - `Block` → return `GuardResult::Blocked`
     - `Redact` → return `GuardResult::Redacted`
     - `Warn` → return `GuardResult::Warned`

3. **PII Filtering**
   - Call `pii::engine::PiiEngine::filter`.
   - Skip if `provider_name` is in the PII config `exclude_providers` list.
   - Forward the redacted text to the next stage.

4. **Content Sanitization** (conditional)
   - If `has_external_content` is true, call `security::content_sanitizer::wrap_external_content`.
   - Otherwise bypass.

5. **Placeholder Replacement**
   - Replace all `{{secret:NAME}}` occurrences with the resolved plaintext values.
   - This is the final step before the text leaves for the LLM API.

### Inbound Pipeline

1. Call `exec::leak_detector::scan_inbound`.
2. Call `secrets::leak_detector::scan_inbound` (uses previously registered injected secrets).
3. Return `GuardResult`.

---

## 5. Mount Points in Agent Loop

### Outbound Hook
- **Location**: `src/agents/run.rs` (or the primary agent-loop driver).
- **Trigger**: After `messages` are fully assembled, but before JSON serialization and HTTP dispatch.
- **Action**: Call `guard.process_outbound` on:
  - The system prompt string.
  - Every `user` and `assistant` message `content` field.

### Inbound Hook
- **Location**: Same file, on the response path.
- **Trigger**: After the LLM response is parsed, but before tool execution or returning to the user.
- **Action**: Call `guard.process_inbound` on:
  - The assistant message `content`.
  - Any `tool_call.arguments` JSON text.

**Constraint**: Changes to `src/agents/run.rs` are minimal — only adding guard invocations, not altering the state machine.

---

## 6. Error Handling Policy

| Scenario | Behavior |
|----------|----------|
| Secret resolution fails (`SecretError::NotFound`) | Return `SecurityGuardError::SecretResolutionFailed`; caller decides abort or degrade. |
| PII filter detects Critical severity | `GuardResult::Blocked` with `redacted_text` for audit/debug. |
| Leak detector finds outbound leak | Respect `default_action_on_leak`: `Block`, `Redact`, or `Warn`. |
| Leak detector finds inbound leak | Prefer `Block` or `Redact` because the LLM may have echoed a user secret. |
| Content sanitizer fails | Return `SecurityGuardError::SanitizationFailed`; default policy is to block (fail-closed). |

---

## 7. Testing Strategy

### Unit Tests (`src/security/runtime_guard.rs`)

- `test_outbound_pipeline_order` — verify stages execute in the correct order.
- `test_secret_injection_then_pii_filter` — injected secrets are still subject to PII filtering.
- `test_leak_detection_blocks_injected_secret_echo` — simulate an LLM echoing an injected secret inbound.
- `test_provider_exclusion_skips_pii` — excluded providers bypass PII filtering.
- `test_guard_result_blocked_preserves_redacted_text` — `Blocked` results include a safe redacted copy for logging.

### Integration Tests (`tests/security_integration.rs`)

- Construct a mock agent-loop request with a `{{secret:NAME}}` placeholder, a PII string, and external content.
- Verify the full outbound → mock LLM → inbound round-trip and assert that `RuntimeSecurityGuard` correctly intercepts leaks.

---

## 8. File Change List

| File | Action | Purpose |
|------|--------|---------|
| `src/security/runtime_guard.rs` | Create | Orchestrator implementation |
| `src/security/mod.rs` | Modify | Export `RuntimeSecurityGuard`, `SecurityGuardConfig`, `GuardResult` |
| `src/agents/run.rs` | Modify | Mount outbound/inbound hooks |
| `src/secrets/mod.rs` | Modify | Ensure `InjectedSecret` and `LeakDetector` are publicly visible |
| `tests/security_integration.rs` | Create | Integration test for the guard |

---

## 9. Success Criteria

- `cargo check -p alephcore --lib` passes with zero new warnings.
- `cargo test -p alephcore --lib` passes; new unit and integration tests cover:
  - All four outbound stages.
  - Both inbound leak-detection paths.
  - Error paths (`SecretResolutionFailed`, `SanitizationFailed`).
- No existing security submodules have their internals changed.
- Agent-loop state-machine logic remains untouched except for added guard calls.

---

## 10. Roadmap Context

This document covers **Phase 1** of a 3-phase plan:

- **Phase 1** (this design): Runtime Security Orchestrator — unify existing security modules into a coordinated pipeline.
- **Phase 2**: LLM Context Protection — hash/redact session identifiers in system prompts, enhance media placeholders, and add platform-aware PII policies.
- **Phase 3**: Audit & Leak Detection Hardening — unified security-audit events, metrics/tracing spans, and comprehensive leak-detection coverage.

---

## 11. Spec Self-Review Notes

- **Placeholder scan**: No "TBD" or "TODO" remain.
- **Internal consistency**: Pipeline order (secret injection → PII → sanitizer → leak detection) is logically sound; sanitization must happen after PII so boundary markers don't wrap raw secrets.
- **Scope check**: This spec is focused enough for a single implementation plan. Vault internals, new PII rules, and prompt-level hashing are intentionally deferred to Phase 2.
- **Ambiguity check**: The fail-closed policy for sanitizer failures is explicitly stated. Mount-point location (`src/agents/run.rs`) may be refined during implementation if the primary loop driver is in a slightly different file, but the contract (after assembly, before dispatch) is unambiguous.
