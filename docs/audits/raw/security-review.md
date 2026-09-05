# src/security review (raw agent output)

## Summary
- Files scanned: src/security/{mod,audit,audit_drain,content_sanitizer,dangerous_tools,headers,injection_patterns,runtime_guard,safe_regex,secret_env,secret_equal,unicode_guard}.rs and src/security/ssrf/{mod,dns,fetch,hostname,ip,policy}.rs
- Critical: 0, Important: 9, Minor: 8
- Health: orange

## Strengths
- secret_equal_bytes correctly uses subtle::ConstantTimeEq with pad-to-equal-length
- SSRF stack is layered defensively (hex/octal/decimal IPv4, punycode, IPv6 transitions)
- Content sanitizer pipeline is order-correct (homoglyph fold → invisible strip → fence-prefix escape → forged-marker detection)
- Audit module has AUTHORITY_PRODUCERS census that bidirectionally asserts producer/consumer
- runtime_guard::process_outbound correctly closes the post-substitution leak window by re-scanning after secrets are resolved

## Critical findings
None.

## Important findings

### I-1 `headers` CSP-wildcard detection misses `*;` and `*` as terminator
- File: src/security/headers.rs:127-145
- Problem: A CSP like `default-src *;frame-ancestors 'none'` contains none of the detector's substrings ("* ", " *;", or ending with " *"). A real wildcard-source CSP is silently emitted on the wire.
- Suggested fix: Generalize the wildcard scan to any standalone `*` token.

### I-2 `secret_equal` API surface permits `("", "") → true` auth bypass
- File: src/security/secret_equal.rs:67-83
- Problem: `secret_equal_bytes(b"", b"")` returns true by design. `secret_equal(Some(""), Some(""))` returns true. Other callers relying on whatever they pass through get an auth bypass for misconfigured empty secrets.
- Suggested fix: In secret_equal, special-case `provided == Some("") || expected == Some("")` to return false before delegating.

### I-3 SSRF block has no audit-trail entry
- File: src/security/ssrf/{fetch,hostname,ip,dns,mod}.rs + src/runtime_guard.rs
- Problem: safe_fetch / validate_url_with_pinned can return SsrfError::BlockedAddress but neither produces an AuditEntry.
- Suggested fix: Add a new AuditEventType::SsrfBlocked and have validate_url_full/validate_url_with_pinned emit one whenever a block decision is made.

### I-4 Audit log fail-open + small buffer + opaque drop counter
- File: src/security/audit.rs:264-294 + src/security/audit_drain.rs
- Problem: SecurityAuditLog::log uses try_send, on Full it increments dropped_count. The 256-buffer channel will fill and silently drop. dropped_count is pub(crate) — no external observer can read it.
- Suggested fix: (1) expose dropped_count via a getter, (2) wire a [audit] config knob block_on_full, (3) emit a dedicated AuditEntry on first drop.

### I-5 `is_blocked_hostname` does not catch Unicode homographs in non-punycode input
- File: src/security/hostname.rs:38-65
- Problem: The punycode/homograph chain runs only if lower.contains("xn--"). A hostname like `localhоst` (Cyrillic U+043E) returns false.
- Suggested fix: Run normalize_homoglyphs unconditionally on the lower-cased input.

### I-6 `safe_fetch` redirect body semantics diverge from RFC 7231 §6.4 for 303
- File: src/security/ssrf/fetch.rs:248-262
- Problem: The body-drop condition keeps the body on 303 for non-POST methods. RFC 7231 §6.4.4 says 303 indicates a fresh GET.
- Suggested fix: Drop the body on 303 regardless of method.

### I-7 `RuntimeSecurityGuard::new` silently disables audit
- File: src/security/runtime_guard.rs:78-85
- Problem: new() accepts audit_enabled: true and forcibly sets it to false before constructing the guard.
- Suggested fix: Either rename `new()` to `new_without_audit()` or consume a config type without an audit_enabled field.

### I-8 `injection_patterns::scan` / `first_threat_message` has no canonicalize path for raw text
- File: src/security/injection_patterns.rs:281-294
- Problem: scan and first_threat_message are pub, miss zero-width-split or Cyrillic-folded payloads. The canonicalizing variant first_threat_message_canonicalized is pub(crate).
- Suggested fix: Demote first_threat_message and scan to pub(crate). Re-export only first_threat_message_canonicalized.

### I-9 Audit `detail` field has no length bound or sanitization
- File: src/security/audit.rs:130-145
- Problem: AuditEntry::detail: String with no length cap, no newline/control-character stripping. Multi-line secrets or large payloads inflate the audit table unboundedly.
- Suggested fix: Add a detail_sanitize helper that collapses internal newlines and caps at 4 KiB with trailing truncation marker.

## Minor findings
### M-1 process_inbound returns Redacted without running PII on the redacted text
- File: src/security/runtime_guard.rs:347-365

### M-2 process_outbound re-locks the same mutex three times in step 5
- File: src/security/runtime_guard.rs:266-292

### M-3 scrub_special_tokens is O(N×M) with ~30 markers
- File: src/security/content_sanitizer.rs:341-378

### M-4 is_blocked_ipv4 doesn't block 192.0.0.0/24 or 198.97.0.0/15
- File: src/security/ssrf/ip.rs:13-72

### M-5 is_invisible_char doesn't cover U+180E or U+034F
- File: src/security/unicode_guard.rs:23-72

### M-6 audit_drain swallows Ok(Ok(())) silently on every successful insert
- File: src/security/audit_drain.rs:54-58

### M-7 headers::is_static_asset recognizes only /assets/ and hardcoded extension list
- File: src/security/headers.rs:46-50

### M-8 safe_fetch redirect loop detection is exact-string
- File: src/security/ssrf/fetch.rs:241-247

## Cross-cutting observations
- Producer/consumer hygiene is good (AUTHORITY_PRODUCERS census)
- Fail-open posture is consistent except I-4 (audit subsystem)
- Lock discipline is correctly documented
- Inconsistency between injection_patterns::scan and first_threat_message_canonicalized is the canonical API footgun
- FencedText and split_external_fence are wired but truncate_sanitized_external_content is unused dead code
