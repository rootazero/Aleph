# Agent Secret Management Phase2/3 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Complete IronClaw-style host-boundary secret controls in Aleph by finishing Phase 1 gaps and delivering Phase 2 (placeholder + leak detection) and Phase 3 (approval + Web3 signing) capabilities.

**Architecture:** Keep `SecretVault` as the only persisted secret source. Add a host-side secret boundary pipeline around outbound/inbound traffic (resolve placeholder -> inject at send time -> leak scan before/after I/O -> never persist plaintext). Reuse existing permission and approval infrastructure for high-risk secret usage and signing flows.

**Tech Stack:** Rust (`alephcore`), `SecretVault` (`core/src/secrets`), `PiiEngine` (`core/src/pii`), `SecretMasker` (`core/src/exec/masker.rs`), Gateway RPC handlers, existing approval manager (`core/src/permission`, `core/src/gateway/handlers/exec_approvals.rs`)

**Design Docs:**
- `docs/plans/2026-02-22-agent-secret-management-design.md`
- `docs/plans/2026-02-22-agent-secret-management-impl.md`

---

### Task 1: Phase 1 Gap Closure - Secret CLI Commands

**Files:**
- Create: `core/src/bin/aleph_server/commands/secret.rs`
- Modify: `core/src/bin/aleph_server/cli.rs`
- Modify: `core/src/bin/aleph_server/commands/mod.rs`
- Modify: `core/src/bin/aleph_server/main.rs`
- Test: `core/src/bin/aleph_server/commands/secret.rs` (unit tests)

**Step 1: Write the failing test**

Add unit tests for secret name normalization and command dispatch helpers:

```rust
#[test]
fn test_secret_name_rejects_empty() {
    assert!(validate_secret_name("").is_err());
    assert!(validate_secret_name("   ").is_err());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib --offline secret_name_rejects_empty -- --exact`
Expected: FAIL because helpers/command module do not exist yet.

**Step 3: Implement minimal command surface**

Add `secret` subcommands (`init`, `set`, `list`, `delete`, `verify`) that call `SecretVault` APIs and never print plaintext:

```rust
pub enum SecretAction {
    Init,
    Set { name: String, value: Option<String> },
    List,
    Delete { name: String },
    Verify { name: String },
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib --offline secret_name_rejects_empty -- --exact`
Expected: PASS.

**Step 5: Commit**

```bash
git add core/src/bin/aleph_server/cli.rs core/src/bin/aleph_server/commands/mod.rs core/src/bin/aleph_server/commands/secret.rs core/src/bin/aleph_server/main.rs
git commit -m "secrets: add aleph-server secret CLI commands"
```

---

### Task 2: Phase 1 Gap Closure - Generation Provider Secret Parity

**Files:**
- Modify: `core/src/config/types/generation/provider.rs`
- Modify: `core/src/gateway/handlers/generation_providers.rs`
- Modify: `core/ui/control_plane/src/api.rs`
- Test: `core/src/gateway/handlers/generation_providers.rs` (unit tests)

**Step 1: Write the failing test**

Add test that generation provider create accepts `secret_name` without plaintext `api_key`:

```rust
#[test]
fn test_generation_provider_secret_name_only_is_valid() {
    let mut cfg = GenerationProviderConfig::new("openai");
    cfg.api_key = None;
    cfg.secret_name = Some("gen_openai_key".to_string());
    assert!(cfg.validate("openai").is_ok());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib --offline generation_provider_secret_name_only_is_valid -- --exact`
Expected: FAIL (field missing / validation mismatch).

**Step 3: Implement minimal parity**

Add `secret_name: Option<String>` and mirror provider handler logic:
- if `api_key` present: write to vault, persist `secret_name`, clear `api_key`
- save config via redaction path before disk write

```rust
pub secret_name: Option<String>
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib --offline generation_provider_secret_name_only_is_valid -- --exact`
Expected: PASS.

**Step 5: Commit**

```bash
git add core/src/config/types/generation/provider.rs core/src/gateway/handlers/generation_providers.rs core/ui/control_plane/src/api.rs
git commit -m "secrets: add secret_name support for generation providers"
```

---

### Task 3: Phase 2 - Secret Placeholder Parsing and Validation

**Files:**
- Create: `core/src/secrets/placeholder.rs`
- Modify: `core/src/secrets/mod.rs`
- Test: `core/src/secrets/placeholder.rs` (unit tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_extract_placeholders() {
    let text = "Bearer {{secret:openai_main_api_key}}";
    let refs = extract_secret_refs(text).unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].name, "openai_main_api_key");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib --offline extract_placeholders -- --exact`
Expected: FAIL because parser is not implemented.

**Step 3: Implement minimal parser**

Support canonical pattern `{{secret:NAME}}`, reject malformed input, and return deterministic order.

```rust
pub struct SecretRef { pub name: String, pub raw: String }
pub fn extract_secret_refs(input: &str) -> Result<Vec<SecretRef>, SecretError> { ... }
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib --offline extract_placeholders -- --exact`
Expected: PASS.

**Step 5: Commit**

```bash
git add core/src/secrets/placeholder.rs core/src/secrets/mod.rs
git commit -m "secrets: add placeholder parser for host-side secret references"
```

---

### Task 4: Phase 2 - Host-Side Secret Injection Pipeline

**Files:**
- Create: `core/src/secrets/injection.rs`
- Modify: `core/src/secrets/mod.rs`
- Modify: `core/src/providers/http_provider.rs`
- Test: `core/src/secrets/injection.rs` (unit tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_inject_placeholders_from_vault() {
    let input = "Authorization: Bearer {{secret:openai_main_api_key}}";
    let rendered = render_with_secrets(input, &resolver).unwrap();
    assert!(!rendered.contains("{{secret:"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib --offline inject_placeholders_from_vault -- --exact`
Expected: FAIL because resolver/renderer does not exist.

**Step 3: Implement minimal injection middleware**

Create helper to resolve placeholders at runtime only:

```rust
pub fn render_with_secrets(input: &str, resolver: &dyn SecretResolver) -> Result<String, SecretError> { ... }
```

Integrate in `HttpProvider::execute` and `execute_stream` right before request build.

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib --offline inject_placeholders_from_vault -- --exact`
Expected: PASS.

**Step 5: Commit**

```bash
git add core/src/secrets/injection.rs core/src/secrets/mod.rs core/src/providers/http_provider.rs
git commit -m "secrets: inject placeholder secrets at host boundary before outbound requests"
```

---

### Task 5: Phase 2 - Bidirectional Leak Detection and Block Policy

**Files:**
- Create: `core/src/secrets/leak_detector.rs`
- Modify: `core/src/secrets/mod.rs`
- Modify: `core/src/providers/http_provider.rs`
- Modify: `core/src/exec/masker.rs`
- Test: `core/src/secrets/leak_detector.rs` (unit tests)

**Step 1: Write the failing test**

```rust
#[test]
fn test_blocks_response_echoing_secret_value() {
    let output = "Your key is sk-ant-abcdefghijklmnopqrstuvwxyz";
    assert!(detector.detect(output).has_blocking_match());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib --offline blocks_response_echoing_secret_value -- --exact`
Expected: FAIL because detector is not wired.

**Step 3: Implement minimal detector**

- Combine rule-based detection (`SecretMasker`) and exact-value detection for recently injected secrets.
- Add request-before-send and response-before-return hooks in `HttpProvider`.
- On detection: return typed error and redact logs.

```rust
pub enum LeakDecision { Allow, Block { reason: String } }
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib --offline blocks_response_echoing_secret_value -- --exact`
Expected: PASS.

**Step 5: Commit**

```bash
git add core/src/secrets/leak_detector.rs core/src/secrets/mod.rs core/src/providers/http_provider.rs core/src/exec/masker.rs
git commit -m "secrets: add bidirectional leak detection at provider I/O boundary"
```

---

### Task 6: Phase 3 - Sensitive Secret Approval Workflow

**Files:**
- Modify: `core/src/permission/rule.rs`
- Modify: `core/src/permission/manager.rs`
- Modify: `core/src/gateway/handlers/exec_approvals.rs`
- Create: `core/src/gateway/handlers/secret_approvals.rs`
- Modify: `core/src/gateway/handlers/mod.rs`
- Modify: `core/src/bin/aleph_server/commands/start.rs`
- Test: `core/src/gateway/handlers/secret_approvals.rs`

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn test_secret_use_can_require_approval() {
    let action = evaluate_secret_use("wallet_sign", "wallet_main").await;
    assert!(matches!(action, PermissionAction::Ask));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib --offline secret_use_can_require_approval -- --exact`
Expected: FAIL due missing secret permission path.

**Step 3: Implement minimal approval bridge**

- Define permission namespace `secret.use`.
- Add RPC handlers: `secret.approval.request`, `secret.approval.resolve`, `secret.approvals.pending`.
- Reuse existing approval storage/timeout style from exec approvals.

```rust
let permission = "secret.use";
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib --offline secret_use_can_require_approval -- --exact`
Expected: PASS.

**Step 5: Commit**

```bash
git add core/src/permission/rule.rs core/src/permission/manager.rs core/src/gateway/handlers/secret_approvals.rs core/src/gateway/handlers/mod.rs core/src/bin/aleph_server/commands/start.rs core/src/gateway/handlers/exec_approvals.rs
git commit -m "secrets: add approval workflow for sensitive secret usage"
```

---

### Task 7: Phase 3 - Web3 Signing Module (No Private Key Exposure)

**Files:**
- Create: `core/src/secrets/web3_signer.rs`
- Modify: `core/src/secrets/mod.rs`
- Modify: `core/src/gateway/handlers/poe.rs`
- Test: `core/src/secrets/web3_signer.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn test_signing_uses_secret_name_without_exposing_key() {
    let signed = signer.sign_intent("wallet_main_key", &intent).unwrap();
    assert!(!signed.debug_text.contains("private_key"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p alephcore --lib --offline signing_uses_secret_name_without_exposing_key -- --exact`
Expected: FAIL because signer module is missing.

**Step 3: Implement minimal signing service**

- Read private key from vault by `secret_name`.
- Sign intent in host process.
- Return signed payload only.

```rust
pub trait Web3Signer {
    fn sign_intent(&self, secret_name: &str, intent: &SignIntent) -> Result<SignedIntent, SecretError>;
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test -p alephcore --lib --offline signing_uses_secret_name_without_exposing_key -- --exact`
Expected: PASS.

**Step 5: Commit**

```bash
git add core/src/secrets/web3_signer.rs core/src/secrets/mod.rs core/src/gateway/handlers/poe.rs
git commit -m "secrets: add host-side web3 signer with secret_name references"
```

---

### Task 8: End-to-End Security Verification and Rollout Gates

**Files:**
- Create: `core/tests/secret_boundary_integration.rs`
- Create: `core/tests/secret_approval_integration.rs`
- Modify: `docs/SECURITY.md`

**Step 1: Write the failing integration test**

Add one red-path and one green-path:
- red: injected secret is echoed by mock response -> blocked
- green: placeholder resolved, request succeeds, no plaintext persisted

**Step 2: Run tests to verify current failure**

Run: `cargo test -p alephcore --offline secret_boundary_integration -- --nocapture`
Expected: FAIL before full wiring.

**Step 3: Implement/adjust integration hooks**

Complete missing wiring to make tests pass (detector hooks + approval callbacks + no-persist checks).

**Step 4: Run full verification**

Run:
```bash
cargo test -p alephcore --lib --offline
cargo test -p alephcore --offline secret_boundary_integration -- --nocapture
cargo test -p alephcore --offline secret_approval_integration -- --nocapture
cargo check -p alephcore --bin aleph-server --offline
```

Expected: all commands exit 0.

**Step 5: Commit**

```bash
git add core/tests/secret_boundary_integration.rs core/tests/secret_approval_integration.rs docs/SECURITY.md
git commit -m "secrets: add end-to-end boundary tests and rollout security checks"
```

---

## Milestone Exit Criteria

- No plaintext provider secret is persisted to `~/.aleph/config.toml`.
- `secret_name` is supported for both chat and generation providers.
- Placeholder format `{{secret:NAME}}` resolves only in host boundary.
- Outbound and inbound leak detection blocks known/observed secret leakage.
- High-risk secret usage and signing operations can require explicit approval.
- Web3 signing path never returns private key material to agent/LLM code.

## Rollout Order

1. Deploy Task 1-2 (Phase 1 closure) behind default-safe behavior.
2. Deploy Task 3-5 (Phase 2 boundary) with logging-first mode for 48h, then enforce block.
3. Deploy Task 6-7 (Phase 3 approval/signing) in opt-in mode per workspace profile.
4. Enforce all gates after Task 8 passes in CI and staging.
