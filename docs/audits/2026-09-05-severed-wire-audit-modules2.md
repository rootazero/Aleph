# Severed Wire Audit — 2026-09-05 (Modules 2)

Static code review across 5 modules (no cargo check during review; fixes applied in worktree).

Reviewed: src/secrets, src/security, src/session, src/skill, src/spend
Reviewers: 5 parallel subagents (general-purpose)
Branch: review/secrets-security-session-skill-spend

## Summary

| Module | Critical | Important | Minor | Health |
|--------|---------:|----------:|------:|:-------|
| secrets | 1 | 5 | 6 | yellow |
| security | 0 | 9 | 8 | orange |
| session | 0 | 4 | 5 | green |
| skill | 2 | 4 | 4 | orange |
| spend | 0 | 3 | 6 | green |
| **Total** | **3** | **23** | **29** | — |

## Critical Findings (must fix)

### src/secrets
- `src/secrets/types.rs:78-96` — `SecretError::NotFound` / `InvalidPlaceholder` `Display` impls echo
  the offending `name` verbatim. A model iterating `{{secret:NAME}}` placeholders can
  distinguish vault hits from misses and enumerate the operator's namespace. The
  existing fix in `src/guardrails/pii_secrets.rs:107-118` proves the leak path is real.

### src/skill
- `src/skill/mod.rs:542-577` — `remove_skill` clears `UsageStore` entries but never
  clears `CoOccurrenceLog` entries, which has no `forget` API. A deleted skill keeps
  reappearing in `cluster_chains` workflow proposals for up to `MAX_ENTRIES = 512`
  records.
- `src/skill/manifest.rs:282` — `serde_yaml 0.9.34+deprecated` is the deserializer for
  untrusted skill frontmatter (YAML aliases / type-confusion class CVEs against
  `yaml-rust`). The 1 MiB cap bounds the window but does not eliminate the parser's
  structural exposure; `preprocess.rs:123` (inline-shell opt-in probe) is the higher-
  value target — one parse away from `allow-inline-shell: true`.

## Important Findings (should fix — selection)

### src/secrets
- `src/gateway/security/shared_token.rs:303-321` (`reset_token`) — plaintext entries
  held as plain `String` after `decrypt`, bypassing the zeroize-on-drop invariant.
  Use `SecretString` / `Zeroizing<String>`.
- `src/secrets/leak_detector.rs:269-318` — `injected_hashes` and `injected_lens` are
  independent LRU caches; under churn a registered secret can be half-evicted.
- `src/secrets/leak_detector.rs:387-446` — `find_all_injected_substrings` is
  O(n × |lens|) where `|lens|` is up to 1024. Hot path under `runtime_guard.rs` mutex.
- `src/secrets/leak_detector.rs:472-498` — `redact_all_matches` invariant enforced
  only by `debug_assert!`; release-build panic or silently garbled redaction.

### src/security
- `src/security/headers.rs:127-145` — CSP-wildcard detector misses `*;` terminator.
- `src/security/secret_equal.rs:67-83` — `secret_equal(Some(""), Some(""))` returns
  true, an auth bypass for misconfigured empty secrets.
- `src/security/hostname.rs:38-65` — `is_blocked_hostname` does not catch Unicode
  homographs in non-punycode input (Cyrillic `lоcalhost`).
- `src/security/ssrf/fetch.rs:248-262` — 303 redirect keeps body for non-POST
  methods; diverges from RFC 7231 §6.4.4.
- `src/security/runtime_guard.rs:78-85` — `RuntimeSecurityGuard::new` silently
  disables audit when caller passes `audit_enabled: true`.
- `src/security/injection_patterns.rs:281-294` — `scan` / `first_threat_message`
  are `pub` without canonicalize; `first_threat_message_canonicalized` is `pub(crate)`
  only, so the footgun is loaded for every new caller.

### src/session
- `src/session/actor.rs:156-209` — `EmitEvent` does not reset `idle_deadline`;
  long-running writers (no `GetEvents`/`Subscribe`) time out at minute 30.
- `src/session/in_process.rs:380-446` — `wake_lock` dropped before `spawn_actor`,
  exposing a race that breaks `SessionWoken.seq == prior_head + 1`.
- `src/session/store.rs:875` — `is_event_retired` accessor has zero callers; clear/
  rewind race against projector is unwired.

### src/skill
- `src/utils/atomic_io.rs:53-72` — `with_file_lock` has no timeout; a crashed peer
  blocks `.usage.json` / `.cooccur.json` updates forever. Hot path via `record_use`.

### src/spend
- `src/spend/sqlite.rs:124-167` + `mod.rs:289-305,691-693` — `f64` NaN propagation
  in `usd` accumulation silently lets spend bypass the ceiling
  (`ceiling_blown(NaN, X) == false`).
- `src/spend/mod.rs:323-350,368-408` — `InMemorySpendLedger::total_for` /
  `principals_in` are O(N) over all retained periods; hot path under every
  embedded-mode `check_with`.

## Findings explicitly NOT addressed in this pass

### Critical / Important skipped (architectural / scope-sensitive)
- `security I-3` (SSRF no audit trail) — needs new `AuditEventType` variant +
  producer census + drain wiring; large cross-module surface.
- `security I-4` (audit log silent fail-open) — needs config knob + blocking
  send + new `AuditLogDropped` variant; user-facing semantics change.
- `security I-9` (audit detail length bound) — touches every audit producer.
- `secrets I-3` (`OnePasswordProvider` stub) — boundary decision (delete vs wire)
  is product-level, not a static-review fix.
- `skill I-1` (companion-file scanning in `parse_skill_file`) — needs directory
  walk + size cap; behavior change at install boundaries.
- `skill I-3` (`preprocess.rs` inline-shell) — needs sandbox design.
- `skill I-4` (`compat::SkillInfo` projection) — schema-touching.
- `session I-4` (cross-backend equivalence test) — needs fixture + test infra.
- `spend I-1` (`total_for` zero after policy change) — touches policy + ledger
  migration; high regression risk for an offline-only behavior.

These are recorded in the raw reports under `docs/audits/raw/*.md` for a future
pass.

### Minor findings
All Minor findings documented in the raw reports; none addressed in this pass
per the dispatcher's "don't manufacture findings; only fix what is actionable"
guidance and the user's "fix directly on main" instruction.

## Cross-cutting observations

- **Zeroize boundary discipline**: held well in `secrets::crypto` and
  `DecryptedSecret`; violated at the boundary in `shared_token.rs::reset_token`.
- **Fail-open posture**: consistent across `spend`, `runtime_guard`, `audit_drain`;
  the `security/audit` subsystem is the lone place where silent degradation is
  invisible to operators (a candidate for a future pass).
- **Public-vs-pub(crate) asymmetry**: `security/injection_patterns::scan` /
  `first_threat_message` are `pub` while the canonicalize-required variants are
  `pub(crate)` — the exact shape of footgun the canonicalize discipline was built
  to prevent.
- **Capability-slot discipline**: consistent in `session::service` and
  `session::store`; the `is_event_retired` row is the only half-wired slot.
- **No `as u64` on `i64` sums**: the 2026-09-05 audit caught this pattern in
  `resilience/database`; `spend/sqlite.rs` has it again on `unpriced_calls` /
  `partial_calls`. Safe today but the same latent corruption vector.
- **`tracing::warn!` is rare**: poisoned-mutex recovery is uniformly silent
  (`unwrap_or_else(|e| e.into_inner())`) across `spend`, `secrets`,
  `runtime_guard`. A project-wide convention here would help.

## Process notes

- Reviewers dispatched in parallel from `.worktrees/sev-wire-modules2/`.
- No `cargo check` during review (per user instruction); single unified check
  after all fixes land.
- Memory control on the final `cargo check`:
  `CARGO_BUILD_JOBS=2 CARGO_PROFILE_DEV_DEBUG=1 cargo check -p alephcore`.

## Raw reports

- docs/audits/raw/secrets-review.md
- docs/audits/raw/security-review.md
- docs/audits/raw/session-review.md
- docs/audits/raw/skill-review.md
- docs/audits/raw/spend-review.md