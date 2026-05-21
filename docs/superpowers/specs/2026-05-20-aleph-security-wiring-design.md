# Aleph Security Wiring — Cycle Design

**Date**: 2026-05-20
**Scope**: Wire existing-but-unwired security infrastructure end-to-end; byte-level secret scrub at sandbox edge; input-side hardening verification.
**Prior art**:
- [`2026-04-16-runtime-security-orchestrator-design.md`](./2026-04-16-runtime-security-orchestrator-design.md) — designed `RuntimeSecurityGuard` (the orchestrator). **This cycle wires what that spec designed.**
- [`2026-04-27-runtime-security-enhancement-design.md`](./2026-04-27-runtime-security-enhancement-design.md) — gap analysis (clawshell comparison).
- [`2026-04-25-p3-guardrails-design.md`](./2026-04-25-p3-guardrails-design.md) — current `GuardrailRegistry` + `PiiSecretsGuardrail` Phase-3 design.
- Reference project: `/Volumes/TBU4/Github/clawshell` — HTTP DLP proxy (not an agent shell; mis-named in original request). Reused ideas: byte-level regex DLP, virtual-key-style placeholder indirection.

**Non-goals**: New PII rules; sandbox OS-level changes (see `2026-05-20-sandbox-hardening-cycle1-design.md`); streaming-time chunk-wise DLP; ToolPipeline architectural change.

---

## 1. The Problem

Aleph has two parallel security stacks for runtime evaluation:

- **`RuntimeSecurityGuard`** (`src/security/runtime_guard.rs`, 579 lines) — the *intended* unified orchestrator: placeholder → secret resolve → leak scan × 2 → content sanitize → PII filter → audit. Designed in [2026-04-16 spec](./2026-04-16-runtime-security-orchestrator-design.md). **Has no production caller** — `src/security/mod.rs:26` shows `let _ = RuntimeSecurityGuard::default_guard();` which discards the result, a literal no-op.
- **`PiiSecretsGuardrail`** (`src/guardrails/pii_secrets.rs`, 101 lines) — the *actually wired* lightweight trait adapter: `leak.scan_outbound + pii.filter`. Wired at boot via `[guardrails]` config → `orchestrator_init.rs:215` → `HarnessDeps.guardrail_registry`. Triple-implements `InputGuardrail + OutputGuardrail + ToolCallGuardrail` but **does no placeholder substitution and no audit logging**.

Result: every feature the 2026-04-16 spec promised — placeholder injection, content sanitization, secret-hash-aware leak detection, audit trail — is dead in production. Each tool call passes through `PiiSecretsGuardrail::evaluate_tool_call` which checks for already-leaked secrets but cannot resolve `{{secret:NAME}}` placeholders. Models cannot use vault references safely; the only working secret path is the LLM literally seeing the plaintext.

Concrete consequences:

| Symptom | Root cause |
|---|---|
| LLM transcripts contain plaintext `sk-ant-...` because there's no mechanism to use `{{secret:openai_main}}` in tool args | `render_with_secrets` has zero production callers (`grep` confirms) |
| `security_audit_log` SQL table exists with full schema but is empty | `SecurityAuditLog`'s `mpsc::Receiver<AuditEntry>` is never consumed; the table has no writer |
| `executor/builtin_registry/{registry.rs:64,419,425, builder/constructor.rs:39}` carry 4 stale `TODO: ... following OpenClaw's sandbox/tool-policy pattern` markers from a migration that never happened | Tool policy is already implemented elsewhere (Sandbox + GuardrailRegistry + ApprovalGate); TODOs mislead future contributors |
| `bash_exec` stdout/stderr containing non-UTF-8 bytes around secret patterns escape leak detection | `regex::Regex` (str-based) runs on `String::from_utf8_lossy(stdout)` — bad bytes become `U+FFFD`, breaking secret patterns |
| Pasting `sk-proj-XXX` into a user message: behavior unverified | `PiiSecretsGuardrail::evaluate_input` calls `leak.scan_outbound` which is named for outbound flow; coverage on input direction lacks explicit tests |

This cycle does not invent new mechanisms. It wires the existing orchestrator and closes five concrete defects.

---

## 2. Goals

1. **G1** — Unify the two parallel security stacks: `PiiSecretsGuardrail` becomes a thin trait adapter that delegates to `RuntimeSecurityGuard`. No new orchestration; one source of security truth.
2. **G2** — Activate placeholder substitution end-to-end: `{{secret:NAME}}` in LLM-generated tool args resolves to real values **only** at the sandbox boundary, never in user-visible transcripts.
3. **G3** — Drain `SecurityAuditLog` to the pre-existing `security_audit_log` SQL table so audit events stop dropping on the floor.
4. **G4** — Scrub secret patterns at the byte level on `Sandbox` stdout/stderr **before** UTF-8 conversion, closing the non-UTF-8 escape route.
5. **G5** — Verify (and harden) input-side blocking of pasted API keys; cover `sk-proj`, `sk-ant`, `AKIA`, `ghp_`, `glpat-` prefixes with explicit tests.
6. **G6** — Clean up dead markers: drop `let _ = …default_guard()` boot no-op; replace the four `OpenClaw tool-policy` TODOs with reverse-pointer doc comments.

Done bar: `cargo test -p alephcore --lib` green with no new regressions; one manual e2e (bash_exec called with a `{{secret:dummy_test_key}}` placeholder, transcript shows the placeholder, sandbox sees the resolved value, audit row written); CHANGELOG updated for the next release.

---

## 3. Architecture

### 3.1 Component shape after the cycle

```
HarnessDeps.guardrail_registry: Arc<GuardrailRegistry>
└─ inner.{input,output,tool_call}: Vec<Arc<dyn …Guardrail>>
    └─ PiiSecretsGuardrail  (trait adapter — surface unchanged)
        └─ inner: Arc<RuntimeSecurityGuard>            ← single backend
            ├─ placeholder.extract_secret_refs
            ├─ AsyncSecretResolver.resolve            ← injected at boot
            ├─ secret_leak_detector.register_injected
            ├─ exec_leak_detector + secret_leak_detector.scan
            ├─ content_sanitizer (external-content wrap)
            ├─ pii_engine.filter
            └─ audit_log → mpsc::Sender<AuditEntry>
                            │
                            ▼
                    AuditDrainTask  (new)
                    tokio::spawn at boot → INSERT into security_audit_log

WorkspaceSandbox.run_command pipeline (src/sandbox/workspace.rs)
└─ step 5 "run": OsSandboxDriver returns (exit_code, stdout_bytes: Vec<u8>, stderr_bytes: Vec<u8>)
└─ step 5.5 "scrub" (new):
    scrub_secrets_bytes(&stdout_bytes, &context.injected_secrets) → ScrubResult
    scrub_secrets_bytes(&stderr_bytes, &context.injected_secrets) → ScrubResult
└─ step 6 "audit": existing (now also records scrub stats)
└─ String::from_utf8_lossy → caller (already enters OutputGuardrail downstream)
```

### 3.2 Three guardrail surfaces, three resolver strategies

| Surface | `RuntimeSecurityGuard::process_outbound` called with… | Why |
|---|---|---|
| `evaluate_input(user_text)` | `resolver: None` | User-typed text. If we resolved placeholders here we would let a malicious user query vault contents via prompt. Leak + PII only. |
| `evaluate_output(llm_text)` | `resolver: None` | LLM-generated text destined for the user. Resolving `{{secret:foo}}` here would surface plaintext secrets in the chat UI. Leak + PII only. |
| `evaluate_tool_call(name, args)` | `resolver: Some(&dyn AsyncSecretResolver)` | LLM-generated tool args destined for the sandbox. This is the unique location where placeholder→plaintext transition is correct, because the next consumer is the tool runtime, not the user. |

Substitution happens *exclusively* at the tool-call surface. This is the design's load-bearing claim and the reason the trait surface doesn't need to change.

### 3.3 Sanitize variant carries rendered args back

`GuardrailDecision::Sanitize(Replacement { text, source })` is already in the trait. When `evaluate_tool_call` resolves a placeholder, it returns `Sanitize` with `text` = rendered JSON of args. The existing `ToolCallGuardrail` consumer in the tool pipeline must then parse the rendered JSON back into a `Value` and pass that to the sandbox. We verify the consumer already does this (see Implementation §4.5).

If the consumer does **not** currently honor `Sanitize` for tool args, this cycle adds a minimal change there — but only there, and only to thread the replacement text. No structural rework of the pipeline.

### 3.4 Audit drain

`SecurityAuditLog::new_with_audit` returns a `mpsc::Receiver<AuditEntry>` that today is always discarded. We introduce `src/security/audit_drain.rs::spawn_audit_drain(rx, db_pool)`:

```text
loop {
    match rx.recv().await {
        Some(entry) => insert into security_audit_log VALUES (now, type, severity, ip, session, detail)
                       on error → tracing::error!(error = %e, "audit drain insert failed"), continue
        None        => graceful exit (sender dropped at shutdown)
    }
}
```

Backpressure is already absorbed by `try_send`'s drop-oldest semantics in `SecurityAuditLog::log`. The drain task is single-consumer; one task per server. Schema (`security_audit_log` table) already exists in `src/security/audit.rs:127-138`.

---

## 4. Implementation Plan

### 4.1 File-level change list

| Op | File | Purpose |
|---|---|---|
| ✏️ Edit | `src/guardrails/pii_secrets.rs` | Replace `pii + secrets` fields with `Arc<RuntimeSecurityGuard>` + `Option<Arc<dyn AsyncSecretResolver>>`. Three trait impls call `inner.process_outbound`, choosing resolver per surface. Delete the private `evaluate()` helper (now superseded by the orchestrator). |
| ✏️ Edit | `src/guardrails/pii_secrets.rs` (constructors) | Add `from_globals_with_resolver(resolver: Option<Arc<dyn AsyncSecretResolver>>)`. Keep `from_globals()` as a thin wrapper passing `None` for one release for backward compatibility of any caller; deprecate-comment it. |
| ✏️ Edit | `src/bin/aleph-server/commands/start/orchestrator_init.rs` | Call `from_globals_with_resolver(Some(vault_resolver_arc))`. Spawn `audit_drain` task. Keep audit `rx` alive until server shutdown signal. |
| ✏️ Edit | `src/security/mod.rs` | Delete the `let _ = RuntimeSecurityGuard::default_guard();` boot no-op at line 26. Re-export `spawn_audit_drain`. |
| ➕ New | `src/security/audit_drain.rs` | `pub async fn spawn_audit_drain(rx, pool, shutdown) -> JoinHandle<()>`. SQL `INSERT` uses prepared statement from `audit.rs:147`. |
| ➕ New | `src/sandbox/scrub.rs` | `pub struct ScrubResult { cleaned: Vec<u8>, hits: usize, decision: LeakDecision }`. `pub fn scrub_secrets_bytes(bytes: &[u8], injected: &[InjectedSecret]) -> ScrubResult`. Uses `regex::bytes::Regex` patterns from `default_patterns_bytes()` below. Honors `InjectedSecret` whitelist by hashing matched byte-ranges and skipping known hashes. |
| ✏️ Edit | `src/secrets/leak_detector.rs` | Extract pattern list into `pub fn default_patterns_bytes() -> Vec<bytes::Regex>` so `scrub.rs` and the existing str-based detector share one source of truth. No behavior change to existing str path. |
| ✏️ Edit | `src/sandbox/workspace.rs` | After driver returns raw bytes, run `scrub_secrets_bytes` on stdout + stderr before lossy UTF-8 conversion. Plumb `SecurityContext.injected_secrets` from the calling guard. If sandbox has no security context (e.g., direct CLI invocation), scrub still runs but with empty `injected` slice (defaults to plain detection). |
| ✏️ Edit | `src/executor/builtin_registry/registry.rs` | Delete TODO comments at lines 64, 419, 425. Replace at line 64 with `/// Security enforcement is layered: GuardrailRegistry (input/output/tool-call) + WorkspaceSandbox (OS isolation) + ApprovalGate (HITL escalation). See docs/reference/SANDBOX.md and docs/reference/SECURITY.md.` |
| ✏️ Edit | `src/executor/builtin_registry/builder/constructor.rs:39` | Same TODO cleanup. |
| ➕ New tests | `src/guardrails/pii_secrets.rs::tests` | (a) input-side blocks `sk-proj-…`, `sk-ant-…`, `AKIA…`, `ghp_…`, `glpat-…`; (b) input never resolves `{{secret:foo}}`; (c) output never resolves `{{secret:foo}}`; (d) tool_call resolves `{{secret:foo}}` and returns `Sanitize` with rendered text. |
| ➕ New tests | `src/sandbox/scrub.rs::tests` | (a) non-UTF-8 bytes around a `sk-` literal still match; (b) known `InjectedSecret` hashes are whitelisted; (c) empty input is no-op. |
| ➕ New tests | `src/security/audit_drain.rs::tests` | (a) entries flushed to mock pool; (b) graceful shutdown when sender drops; (c) SQL error does not stop drain. |
| ➕ Integration test | `tests/security_wiring_integration.rs` | End-to-end: build harness, call ToolCallGuardrail with `{ "command": "echo {{secret:test_key}}" }`, mock resolver returns `"resolved-value"`; assert rendered args, assert injected_secrets count, assert audit row queued. |

### 4.2 `RuntimeSecurityGuard` API — no changes

The orchestrator's `process_outbound(text, resolver, context) -> GuardResult` already supports `resolver: Option<_>`. When `None`, it skips Step 1 (placeholder extraction). No API change required.

### 4.3 `AsyncSecretResolver` impl

A `VaultSecretResolver` already exists (verify in `src/secrets/vault.rs` or compose from `SecretResolver` trait in `web3_signer.rs`). If a vault-backed async resolver is missing, we add a 30-line shim:

```rust
pub struct VaultSecretResolver { vault: Arc<SecretVault> }

#[async_trait]
impl AsyncSecretResolver for VaultSecretResolver {
    async fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError> {
        self.vault.get(name).await  // existing API
    }
}
```

### 4.4 Error handling

| Failure | Behavior |
|---|---|
| `SecretError::NotFound` during `evaluate_tool_call` | Return `GuardrailDecision::Block { reason: "Secret 'X' not found", class: Fixable }`. Tool pipeline reports back to LLM as a tool error, LLM may retry. |
| Resolver returns transient error | `Block` with class `Retryable`. |
| `audit_drain` SQL insert fails | `tracing::error!`, drop the entry, continue. Never panic. |
| `scrub_secrets_bytes` finds bytes-level match | If `default_action_on_leak == Block`, replace matched range with `[REDACTED:N]`, return `cleaned` bytes with `hits = N`. Caller decides whether to also fail the tool call (workspace.rs returns redacted bytes + flag). |
| Audit channel full | Existing `try_send` drop-oldest semantics — unchanged. |
| Boot-time resolver missing | `from_globals_with_resolver(None)` — orchestrator still runs leak + pii; placeholder Step 1 is skipped with a one-time `tracing::warn!` at first call. |

### 4.5 Sanitize-honoring in tool pipeline

Before changing `evaluate_tool_call` to return `Sanitize(rendered_json)`, we verify that the existing tool-call consumer in the pipeline parses `Sanitize.text` back into `serde_json::Value` for sandbox dispatch. Investigation lives in the plan's first task; if the consumer does not currently honor `Sanitize`, the plan adds a single-spot edit in the consumer to parse and dispatch.

### 4.6 Cleanup checklist (屎山防治)

After the implementation lands, the diff must include these removals; absence of any of them is a review-blocking issue:

- [ ] `PiiSecretsGuardrail::evaluate()` private method
- [ ] `PiiSecretsGuardrail.pii`, `PiiSecretsGuardrail.secrets` fields (replaced by single `inner: Arc<RuntimeSecurityGuard>`)
- [ ] `src/security/mod.rs:26` `let _ = …default_guard();` no-op
- [ ] 4 × `TODO: … OpenClaw … tool-policy` markers (registry.rs ×3, constructor.rs ×1)
- [ ] Old `from_globals()` constructor *only if* no remaining caller; otherwise keep with deprecation note for one release

---

## 5. Testing Strategy

### 5.1 Unit (in-file `#[cfg(test)]` modules)

- `pii_secrets.rs`: 5 new tests covering the surface × resolver matrix.
- `scrub.rs`: 3 new tests covering bytes-level edge cases.
- `audit_drain.rs`: 3 new tests covering the drain loop.

### 5.2 Integration (`tests/security_wiring_integration.rs`)

One scenario, end-to-end:
1. Boot `GuardrailRegistry` with `from_globals_with_resolver(Some(mock_resolver))`.
2. Call `registry.evaluate_tool_call("bash_exec", &json!({ "command": "echo {{secret:test_key}}" }))`.
3. Assert `GuardrailDecision::Sanitize` with text containing `"resolved-value"` and not containing `{{secret:`.
4. Assert mock audit pool received one row with `event_type = SecretInjected` and `session_id` set.

### 5.3 Manual e2e

1. `aleph-server secret set dummy_test_key fake-sk-test123`
2. Start server, open a chat session, invoke bash_exec with args `{"command": "echo {{secret:dummy_test_key}}"}` (force-call via tool slash command if available, else through a small test prompt).
3. Inspect:
   - Chat transcript shows the placeholder literally `{{secret:dummy_test_key}}`.
   - Sandbox-captured stdout is `fake-sk-test123`.
   - `sqlite3 ~/.aleph/data/aleph.db 'SELECT * FROM security_audit_log ORDER BY id DESC LIMIT 1'` returns a recent row referencing this session.

### 5.4 Acceptance gate

- `cargo test -p alephcore --lib` — green; no new failures compared to the pre-existing main baseline (main has known pre-existing `--lib` failures + one deadlocking concurrency test; diff our branch's failures against the recorded baseline before signing off).
- `cargo clippy -p alephcore -- -D warnings` on **changed files only** — project-wide clippy/fmt is not currently clean on main, so do not run a project-wide pass.
- One manual e2e per §5.3.
- `CHANGELOG.md` updated.

---

## 6. CHANGELOG (English, draft)

```
### Added
- Security: wire RuntimeSecurityGuard as the unified backend behind PiiSecretsGuardrail; placeholder substitution at tool-call boundary.
- Security: byte-level secret-leak scrub at sandbox stdout/stderr edge (catches non-UTF-8 binary output).
- Security: persistent audit drain task writes SecurityAuditLog entries to the security_audit_log table.

### Fixed
- Security: PiiSecretsGuardrail input-side now explicitly tested against pasted API keys (sk-proj, sk-ant, AKIA, ghp_, glpat- prefixes).

### Removed
- Dead `let _ = RuntimeSecurityGuard::default_guard()` boot no-op.
- 4 vestigial "OpenClaw tool-policy" TODOs in executor/builtin_registry (replaced with doc-comment pointers to SANDBOX.md and SECURITY.md).
```

---

## 7. Out-of-cycle / future work

- Streaming-time chunk-wise DLP for SSE-style tool result streaming. Requires harness changes; see clawshell `translate.rs` for the reference shape.
- ToolPipeline-level explicit `render_tool_args` step (option Z from brainstorm). Architectural; deferred.
- PII engine full migration to `regex::bytes` (option B from brainstorm Q3). Deferred; current edge-scrub approach handles the realistic case.
- Configurable per-platform leak patterns from `aleph.toml` (deferred from [2026-04-27 spec](./2026-04-27-runtime-security-enhancement-design.md)).
