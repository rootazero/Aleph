# Module: security

**Date**: 2026-07-19
**Reviewers**: 4 parallel agents (security, logic, architecture, quality)

## Summary
- Path: `src/security/` (17 files, ~5.5k LOC)
- Raw issues found: 37
- After filtering (high-confidence only): 6

## High-Confidence Issues (will fix)

### 1. Label injection via `as_label()` — HIGH (security)
- **File**: `src/security/content_sanitizer.rs:51-84`
- **Description**: `as_label()` only escapes `"` to `&quot;`, but never escapes the `<<<EXTERNAL_` / `<<<END_EXTERNAL_` fence sequences that `wrap_external_content_with_report` later escapes on the body. A URL, MCP tool name, etc. containing `<<<END_EXTERNAL_UNTRUSTED_CONTENT` reaches the wrapper header verbatim.
- **Evidence**: Web fetch / MCP / email / webhook / browser content / tool error call-sites pass user-controlled fields directly; no upstream sanitization.

### 2. Audit receiver silently dropped in `new()` — HIGH (logic)
- **File**: `src/security/runtime_guard.rs:98-101`
- **Description**: `new()` calls `new_with_audit()` and discards the receiver via `let _rx = …`. Default config has `audit_enabled: true`, so the mpsc channel is created then immediately closed; every `try_send` returns `Err(TrySendError::Closed)` and the misleading "channel full" log fires forever.
- **Evidence**: `audit.rs:126` increments `dropped_count` on every send error; default config audit_enabled = true.

### 3. HashMap iteration order in placeholder replacement — MEDIUM (logic)
- **File**: `src/security/runtime_guard.rs:304`
- **Description**: Iterating `resolved_map` in arbitrary order can partially consume longer placeholders before shorter prefix matches. Security-sensitive (controls what secret value reaches LLM).
- **Fix**: Sort by raw length descending before replace.

### 4. `validate_url_async` allowlist short-circuit — MEDIUM (logic)
- **File**: `src/security/ssrf/mod.rs:108-128`
- **Description**: `validate_url_async` returns early on allowlist via `validate_url_common` without DNS validation. `safe_fetch::validate_url_full` (`fetch.rs:146-150`) classifies the resolved IP for allowlisted hosts to defeat DNS rebinding. Two public API surfaces disagree.
- **Fix**: After allowlist check, perform DNS validation with `for_allowlisted_host()` policy.

### 5. `is_octal_or_short_ipv4` false-positives — LOW (logic)
- **File**: `src/security/ssrf/hostname.rs:114-141`
- **Description**: Returns true if ANY 2-4-part dot-split component has `len>1` and starts with '0' and is all-digit. False-positives on hostnames like `0123.com` (parts=["0123","com"]).
- **Fix**: Octal requires 4 parts AND all parts match octal pattern.

### 6. `is_decimal_ip_literal` over-broad — LOW (logic)
- **File**: `src/security/ssrf/hostname.rs:110-112`
- **Description**: Flags any dot-free all-digit string with `len>3`, including numeric strings > u32::MAX (5+ digits) which cannot be IPv4.
- **Fix**: Require the value parses as `u32`.

## Skipped Issues (low signal / design choices / high risk)

- **Architecture R7-R9 violations** in `injection_patterns.rs` — design choice; the deterministic classification is intentional defense-in-depth, not LLM-replacement. Removal requires product owner sign-off.
- **Bidirectional cross-layer dep** between `audit_drain.rs` and `gateway/security/store` — refactoring across crates, high risk.
- **File length > 500 lines** in `content_sanitizer.rs`, `runtime_guard.rs`, `ssrf/fetch.rs` — refactoring would touch many call-sites.
- **Function length > 50 lines** — refactoring risk.
- **Documentation gaps** on public APIs — cosmetic.
- **YAGNI violations** (reserved enum variants, unused error variants) — need wider audit of call-sites before removal.
- **`context_id_hasher` no production consumer** — needs wider audit; may be consumed via re-export chain.
- **`dangerous_tools.rs` env-var bypass** — design choice; `ALEPH_GATEWAY_TOOLS_ALLOW` is intentional escape hatch.
- **`unwrap_or_else(|e| e.into_inner())` on Mutex** — defensive idiom already documented in sync_primitives.rs; switching to `?` propagates poison errors that callers cannot handle.
- **Misleading `// SAFETY:` comment in headers.rs:129** — cosmetic.

## Status
- 6 high-confidence issues to fix → queued
- After fix: commit, defer `cargo check` to end of full sweep